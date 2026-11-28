//! Members of the organization and invitations (`members.ts`). Every invitation is an email the auth provider sends,
//! so the defaults are safe: nothing is sent without `invite`, a dry run never sends, and more than one email needs
//! `--yes`.
//! `remove` asks y/N on a terminal and needs `--yes` without one; it is recoverable for 30 days (`recover`).

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::catalog::{MEMBER_COMMANDS, command_rows};
use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{CliError, Error, Result, usage};
use crate::json::{a, js_string, n, s, t};
use crate::style::{Style, ago, capitalize, col, list, pad_end, rows_of, usd};

pub fn members_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus members <command> [options]", style.bold("Usage")),
        String::new(),
    ];
    lines.extend(
        MEMBER_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 52)))),
    );
    lines.push(String::new());
    lines.push(style.dim(
        "Every invitation is an email. Nothing is sent without `invite`; --dry-run never sends; more than one needs --yes.",
    ));
    lines
}

pub fn members_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    let emails: Vec<String> = args.iter().skip(1).cloned().collect();
    if action == "help" || (cmd.has("help") && action == "list" && args.is_empty()) {
        if cmd.json {
            return Ok(cmd.print(&json!({ "commands": command_rows("members", MEMBER_COMMANDS) })));
        }
        cmd.lines(members_help(style));
        return Ok(0);
    }
    match action {
        "list" => {
            let response = cmd.core.get("/v1/orgs/members")?;
            if cmd.json {
                return Ok(cmd.print(&json!({ "members": response.get("members") })));
            }
            let rows: Vec<Vec<String>> = a(&response, "members")
                .iter()
                .map(|member| {
                    vec![
                        s(member, "email")
                            .or_else(|| s(member, "name"))
                            .map(String::from)
                            .unwrap_or_else(|| js_string(member.get("userId"))),
                        js_string(member.get("role")),
                        ago(s(member, "joinedAt").unwrap_or("")),
                        match member.get("spentUsd") {
                            None => style.dim("–"),
                            Some(value) => usd(value.as_f64().unwrap_or(0.0)),
                        },
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[col("Member"), col("Role"), col("Joined"), crate::style::right("Spent")],
                rows_of(rows),
                None,
            ));
            Ok(0)
        }
        "pending" => {
            let path = match cmd.args.flag_set("team") {
                Some(team) => format!("/v1/teams/pending?by=person&team={}", enc(team)),
                None => "/v1/teams/pending?by=person".to_string(),
            };
            let response = cmd.core.get(&path)?;
            if cmd.json {
                return Ok(
                    cmd.print(&json!({ "people": response.get("people"), "invitations": response.get("invitations") }))
                );
            }
            let people = a(&response, "people");
            if people.is_empty() {
                cmd.out(style.dim(
                    "Everyone in your teams has joined. Someone added by an email not in the organization yet shows here until they join.",
                ));
                return Ok(0);
            }
            let one = people.len() == 1;
            cmd.out(format!(
                "{} {} assigned to teams {} signed in yet:",
                people.len(),
                if one { "person" } else { "people" },
                if one { "hasn't" } else { "haven't" }
            ));
            cmd.out("");
            let rows: Vec<Vec<String>> = people
                .iter()
                .map(|person| {
                    vec![
                        js_string(person.get("email")),
                        a(person, "teams")
                            .iter()
                            .map(|team| {
                                format!(
                                    "{}{}",
                                    js_string(team.get("slug")),
                                    if s(team, "role") == Some("lead") { " (lead)" } else { "" }
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                        s(person, "home").map(String::from).unwrap_or_else(|| style.dim("–")),
                        ago(s(person, "since").unwrap_or("")),
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[col("Email"), col("Teams"), col("Home"), col("Added")],
                rows_of(rows),
                None,
            ));
            cmd.out("");
            cmd.out(style.dim(if s(&response, "invitations") == Some("available") {
                "Invite them: nexus members invite --pending --dry-run, then --yes. Nothing is sent until you do."
            } else {
                "This Gateway cannot send invitations (CLERK_SECRET_KEY is not set); invite them from Nexus UI → Members."
            }));
            Ok(0)
        }
        "invite" => {
            let pending = cmd.has("pending");
            if pending && !emails.is_empty() {
                return usage("Give emails or --pending, not both.", None);
            }
            if !pending && emails.is_empty() {
                return usage(
                    "Usage: nexus members invite <email…> | --pending [--role admin] [--dry-run] [--yes]",
                    Some("Run `nexus members pending` to see who is in teams but has not joined."),
                );
            }
            let role = match cmd.flag("role") {
                None | Some("member") => "member",
                Some("admin") => "admin",
                Some(_) => return usage("--role must be member or admin.", None),
            };
            let request = if pending {
                json!({ "pending": true, "role": role })
            } else {
                json!({ "emails": emails, "role": role })
            };
            let retry = format!(
                "nexus members invite {}{}",
                if pending {
                    "--pending".to_string()
                } else {
                    emails.join(" ")
                },
                if role == "admin" { " --role admin" } else { "" }
            );
            let result = send_invitations(cmd, request, cmd.has("dry-run"), cmd.has("yes"), &retry)?;
            if cmd.json {
                return Ok(cmd.print(&result));
            }
            cmd.lines(invitation_lines(style, &result));
            Ok(if n(result.get("counts").unwrap_or(&Value::Null), "failed") != 0.0 {
                1
            } else {
                0
            })
        }
        "removed" => {
            let response = cmd.core.get("/v1/orgs/members/removed")?;
            if cmd.json {
                return Ok(cmd.print(&json!({ "members": response.get("members") })));
            }
            let members = a(&response, "members");
            if members.is_empty() {
                cmd.out(style.dim("Nobody was removed from this organization."));
                return Ok(0);
            }
            let rows: Vec<Vec<String>> = members
                .iter()
                .map(|member| {
                    vec![
                        s(member, "email")
                            .or_else(|| s(member, "name"))
                            .map(String::from)
                            .unwrap_or_else(|| js_string(member.get("userId"))),
                        js_string(member.get("userId")),
                        js_string(member.get("role")),
                        ago(s(member, "removedAt").unwrap_or("")),
                        if member.get("recoverable") == Some(&Value::Bool(true)) {
                            let until = s(member, "recoverableUntil").unwrap_or("");
                            format!("until {}", until.get(..10).unwrap_or(until))
                        } else {
                            style.dim("no, final")
                        },
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[
                    col("Member"),
                    col("User id"),
                    col("Role"),
                    col("Removed"),
                    col("Recoverable"),
                ],
                rows_of(rows),
                None,
            ));
            cmd.out("");
            cmd.out(style.dim("Recover one: nexus members recover <userId> --dry-run, then without --dry-run."));
            Ok(0)
        }
        "remove" => remove(cmd, &emails),
        "recover" => {
            let [user_id] = emails.as_slice() else {
                return usage(
                    "Usage: nexus members recover <userId> [--dry-run]",
                    Some("Run `nexus members removed` to see who can be recovered."),
                );
            };
            let result = cmd.core.post(
                &format!("/v1/orgs/members/{}/recover", enc(user_id)),
                json!({ "dryRun": cmd.has("dry-run") }),
            )?;
            if cmd.json {
                return Ok(cmd.print(&result));
            }
            let who = s(&result, "email")
                .map(String::from)
                .unwrap_or_else(|| js_string(result.get("userId")));
            let role = js_string(result.get("role"));
            let teams: Vec<String> = a(&result, "teams").iter().map(|team| js_string(Some(team))).collect();
            let teams = if teams.is_empty() {
                String::new()
            } else {
                format!(" in {}", teams.join(", "))
            };
            cmd.out(if result.get("dryRun") == Some(&Value::Bool(true)) {
                format!(
                    "{} Dry run: nothing changed. {who} would come back as {role}{teams}.",
                    style.warn
                )
            } else {
                format!(
                    "{} {who} is back as {role}{teams}. Their old tokens and keys stay revoked.",
                    style.ok
                )
            });
            Ok(0)
        }
        other => usage(
            format!("Unknown command `nexus members {other}`."),
            Some("Run `nexus members --help`."),
        ),
    }
}

/// `nexus members remove <email|userId>`: one person, after a y/N on a terminal or with `--yes`. Gateway has no dry run
/// for it, so `--dry-run` reads the members list and changes nothing. A removal is recoverable for 30 days.
fn remove(cmd: &Cmd, args: &[String]) -> Result<i32> {
    let style = cmd.style;
    let [wanted] = args else {
        return usage(
            "Usage: nexus members remove <email|userId> [--dry-run] [--yes]",
            Some("Run `nexus members` to see who is in the organization."),
        );
    };
    let response = cmd.core.get("/v1/orgs/members")?;
    let members = a(&response, "members");
    let by_id: Vec<&Value> = members
        .iter()
        .filter(|member| s(member, "userId") == Some(wanted.as_str()))
        .collect();
    let found = if by_id.is_empty() {
        let email = wanted.trim().to_lowercase();
        members
            .iter()
            .filter(|member| s(member, "email").is_some_and(|candidate| candidate.to_lowercase() == email))
            .collect()
    } else {
        by_id
    };
    let member = match found.as_slice() {
        [member] => *member,
        [] => {
            return Err(CliError::hint(
                format!("{wanted} is not a member of this organization."),
                "`nexus members` lists who is; `nexus members removed` lists who was removed.",
            )
            .into());
        }
        many => {
            let ids: Vec<String> = many.iter().map(|member| js_string(member.get("userId"))).collect();
            return Err(CliError::hint(
                format!("{wanted} names {} members.", many.len()),
                format!("Remove one by user id: {}.", ids.join(", ")),
            )
            .into());
        }
    };
    let user_id = js_string(member.get("userId"));
    let who = s(member, "email")
        .or_else(|| s(member, "name"))
        .map(String::from)
        .unwrap_or_else(|| user_id.clone());
    let role = js_string(member.get("role"));
    if cmd.has("dry-run") {
        if cmd.json {
            return Ok(cmd.print(&json!({
                "dryRun": true, "userId": user_id, "email": member.get("email"), "role": member.get("role"),
            })));
        }
        cmd.out(if role == "owner" {
            format!(
                "{} Dry run: nothing changed. {who} is the owner, who cannot be removed; transfer ownership first.",
                style.warn
            )
        } else {
            format!(
                "{} Dry run: nothing changed. {who} ({role}) would be removed, and their tokens and keys revoked.",
                style.warn
            )
        });
        return Ok(0);
    }
    if !cmd.has("yes") {
        if !cmd.ctx.interactive || cmd.json {
            return usage(
                format!("This removes {who} ({role}) from the organization and revokes their tokens and keys at once."),
                Some(&format!("Run `nexus members remove {wanted} --yes` to go ahead.")),
            );
        }
        let question = format!("Remove {who} ({role}) from the organization? [y/N] ");
        if !cmd.ctx.io.write(&question) {
            cmd.out(&question);
        }
        let answer = cmd.ctx.io.read_line().unwrap_or_default().trim().to_lowercase();
        if answer != "y" && answer != "yes" {
            cmd.out(style.dim("Nothing changed."));
            return Ok(1);
        }
    }
    let removed = cmd
        .core
        .delete(&format!("/v1/orgs/members/{}", enc(&user_id)))
        .map_err(removal_refused)?;
    if cmd.json {
        let mut value = removed;
        if let Value::Object(map) = &mut value {
            map.insert("userId".into(), json!(user_id));
            map.insert("email".into(), member.get("email").cloned().unwrap_or(Value::Null));
        }
        return Ok(cmd.print(&value));
    }
    cmd.out(format!(
        "{} Removed {}. Recover them within 30 days: nexus members recover {user_id}",
        style.ok,
        style.bold(&who)
    ));
    Ok(0)
}

/// Gateway's refusals of a removal, each with its next step.
fn removal_refused(error: Error) -> Error {
    let Error::Core(core) = &error else {
        return error;
    };
    let hint = match (core.status, core.code.as_deref()) {
        (_, Some("owner_role")) => "Transfer ownership to someone else first.",
        (_, Some("self_removal")) => "Ask another owner or admin to remove you.",
        (403, _) => "Ask an owner or admin of your organization.",
        (404, _) => "They are not a member now; `nexus members removed` lists who was removed.",
        (_, Some("not_configured")) => {
            "Remove them in Nexus UI → Members, or ask whoever runs Nexus Gateway to configure its sign-in provider."
        }
        (_, Some("use_nexus_ui")) => {
            "This Gateway removes members of a sign-in provider organization only from Nexus UI → Members."
        }
        (_, Some("clerk_unavailable")) => "Nothing changed; run it again.",
        _ => return error,
    };
    CliError::hint(capitalize(&core.message), hint).into()
}

fn with(request: &Value, entries: &[(&str, Value)]) -> Value {
    let mut value = request.clone();
    if let Value::Object(map) = &mut value {
        for (key, entry) in entries {
            map.insert(key.to_string(), entry.clone());
        }
    }
    value
}

/// Check, then send: a dry run first; with `dry_run` that is all. More than one email needs `yes`; the send names the
/// count the check found, so Gateway refuses if anything changed in between.
pub fn send_invitations(cmd: &Cmd, request: Value, dry_run: bool, yes: bool, retry: &str) -> Result<Value> {
    let check = invitation_call(cmd, with(&request, &[("dryRun", json!(true))]))?;
    let to_send = n(&check, "toSend");
    if dry_run || to_send == 0.0 {
        return Ok(check);
    }
    if to_send > 1.0 && !yes {
        return usage(
            format!("This would send {} invitation emails.", js_string(check.get("toSend"))),
            Some(&format!(
                "Review them: `{retry} --dry-run`; send them: `{retry} --yes`."
            )),
        );
    }
    invitation_call(
        cmd,
        with(
            &request,
            &[
                ("dryRun", json!(false)),
                ("expect", check.get("toSend").cloned().unwrap_or(json!(0))),
            ],
        ),
    )
}

fn invitation_call(cmd: &Cmd, request: Value) -> Result<Value> {
    match cmd.core.post("/v1/orgs/invitations", request) {
        Ok(value) => Ok(value),
        Err(Error::Core(error)) if error.status == 501 => Err(CliError::hint(
            capitalize(&error.message),
            "Or ask whoever runs Nexus Gateway to configure its sign-in provider.",
        )
        .into()),
        Err(Error::Core(error)) if error.code.as_deref() == Some("count_changed") => {
            Err(CliError::hint(capitalize(&error.message), "Run it again: it checks the list first.").into())
        }
        Err(Error::Core(error)) if error.status == 403 => Err(CliError::hint(
            capitalize(&error.message),
            "Ask an owner or admin of your organization.",
        )
        .into()),
        Err(error) => Err(error),
    }
}

/// The next step for people in the auth provider's organization who have no Gateway user yet.
const NOT_SIGNED_IN: &str = "People in the organization who have not signed in need no invitation. Ask them to sign in once (the Nexus app or nexus login); their teams apply then.";

fn status_text(status: &str) -> String {
    match status {
        "would_invite" => "would invite",
        "sent" => "sent",
        "already_member" => "already a member",
        "not_signed_in" => "in the organization, not signed in yet",
        "already_invited" => "already invited",
        "removed" => "removed; recover them with nexus members recover",
        "failed" => "failed",
        other => other,
    }
    .to_string()
}

pub fn invitation_lines(style: &Style, result: &Value) -> Vec<String> {
    let counts = result.get("counts").cloned().unwrap_or(Value::Null);
    let count = |value: f64, word: &str| {
        format!(
            "{} {word}{}",
            crate::style::js_number(value),
            if value == 1.0 { "" } else { "s" }
        )
    };
    let role = js_string(result.get("role"));
    let to_send = n(result, "toSend");
    let failed = n(&counts, "failed");
    let head = if t(result, "dryRun") {
        format!(
            "{} Dry run: nothing was sent. {}",
            style.warn,
            if to_send != 0.0 {
                format!("{} would be invited as {role}.", count(to_send, "email"))
            } else {
                "Nobody would be invited.".to_string()
            }
        )
    } else if failed != 0.0 {
        format!(
            "{} Sent {}; {} failed.",
            style.fail,
            count(n(&counts, "sent"), "invitation"),
            js_string(counts.get("failed"))
        )
    } else {
        format!(
            "{} Sent {} as {role}.",
            style.ok,
            count(n(&counts, "sent"), "invitation")
        )
    };
    let mut lines = vec![head];
    let results = a(result, "results");
    if !results.is_empty() {
        lines.push(String::new());
        let rows: Vec<Vec<String>> = results
            .iter()
            .map(|entry| {
                let status = s(entry, "status").unwrap_or("");
                let text = status_text(status);
                vec![
                    js_string(entry.get("email")),
                    match status {
                        "failed" => style.red(&text),
                        "sent" => text,
                        _ => style.dim(&text),
                    },
                    s(entry, "message")
                        .filter(|m| !m.is_empty())
                        .map(|m| style.dim(m))
                        .unwrap_or_default(),
                ]
            })
            .collect();
        lines.extend(list(
            style,
            &[col("Email"), col("Result"), col("")],
            rows_of(rows),
            None,
        ));
    }
    if t(result, "dryRun") && !t(result, "checkedClerk") {
        lines.push(String::new());
        lines.push(style.dim(
            "Clerk was not checked (no CLERK_SECRET_KEY on Gateway): people already invited count as would-invite.",
        ));
    }
    if n(&counts, "not_signed_in") != 0.0 {
        lines.push(String::new());
        lines.push(style.dim(NOT_SIGNED_IN));
    }
    if failed != 0.0 {
        lines.push(String::new());
        lines.push(
            style.dim("Failed ones were not sent; run the same command again to retry them (sent ones are skipped)."),
        );
    }
    lines
}

/// The emails a team import file names (leads and members), lower-cased.
pub fn file_emails(units: &Value) -> BTreeSet<String> {
    let mut emails = BTreeSet::new();
    for unit in units.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        for person in a(unit, "leads").iter().chain(a(unit, "members")) {
            let email = match person {
                Value::String(email) => Some(email.as_str()),
                other => s(other, "email"),
            };
            if let Some(email) = email.filter(|email| email.contains('@')) {
                emails.insert(email.trim().to_lowercase());
            }
        }
    }
    emails
}
