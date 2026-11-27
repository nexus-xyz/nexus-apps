//! `nexus service-accounts`, as `service-accounts.ts` does it: callers for CI and scripts, and their keys.

use serde_json::{Value, json};

use crate::artifacts::day;
use crate::catalog::{SERVICE_ACCOUNT_COMMANDS, SERVICE_ACCOUNT_KEY_COMMANDS};
use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{CliError, Error, Result, usage};
use crate::json::{a, js_string, obj, s, s_nonempty, t};
use crate::models::duration_expiry;
use crate::style::{
    Row, Style, ago_at, capitalize, col, flex, iso, js_number, kv, list, now_ms, pairs, parse_ms, right,
};

const CREATE_USAGE: &str = "Usage: nexus service-accounts create --name <name> --team <slug> [--owner <user-id>] \
    --expires <YYYY-MM-DD|30d> [--dry-run]";
const KEY_CREATE_USAGE: &str =
    "Usage: nexus service-accounts keys create <account id> --label <label> [--expires <YYYY-MM-DD|30d>]";
const ROTATE_USAGE: &str =
    "Usage: nexus service-accounts keys rotate <account id> <key id> [--label <label>] [--expires <YYYY-MM-DD|30d>]";
const LIST_HINT: &str = "`nexus service-accounts` lists the accounts, their ids and their keys.";
pub(crate) const TOKEN_WARNING: &str = "Copy the token now. Nexus does not show it again.";

/// The command names of `nexus service-accounts help --json`.
pub fn service_account_command_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = SERVICE_ACCOUNT_COMMANDS
        .iter()
        .map(|(name, _)| name.split(' ').next().unwrap_or(name))
        .collect();
    names.push("keys");
    names
}

pub fn service_accounts_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus service-accounts <command> [options]", style.bold("Usage")),
        String::new(),
    ];
    for (prefix, rows) in [("", SERVICE_ACCOUNT_COMMANDS), ("keys ", SERVICE_ACCOUNT_KEY_COMMANDS)] {
        for (name, text) in rows {
            lines.push(format!("  {}", style.cyan(&format!("{prefix}{name}"))));
            lines.push(format!("      {text}"));
        }
    }
    lines.push(String::new());
    lines.push(
        style.dim(
            "A service account is no member and holds no seat: its keys (nxsa_…) are how CI and scripts call Nexus.",
        ),
    );
    lines.push(style.dim(
        "A token is shown once, when a key is issued or rotated. --dry-run applies to create and revoke of an account.",
    ));
    lines
}

fn is_day(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(at, byte)| {
            if at == 4 || at == 7 {
                *byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
}

/// `--expires`: a day (YYYY-MM-DD, UTC, the whole day) or a duration (`30d`, `12h`, a number of days), as an ISO time.
pub fn account_expiry(value: Option<&str>, now: f64) -> Result<String> {
    let expires_at = match value {
        Some(text) if is_day(text) => parse_ms(&format!("{text}T23:59:59Z")).map(iso),
        other => duration_expiry(other.unwrap_or(""), now),
    };
    match expires_at {
        Some(expires_at) => Ok(expires_at),
        None => usage(
            "--expires needs a day or a duration: 2027-01-31, 30d, 12h or a number of days.",
            None,
        ),
    }
}

/// `--expires` of a key command: none when the flag is absent, and a usage error when it has no value.
fn key_expiry(cmd: &Cmd) -> Result<Option<String>> {
    if cmd.flag("expires").is_some() || cmd.has("expires") {
        account_expiry(cmd.flag("expires"), now_ms()).map(Some)
    } else {
        Ok(None)
    }
}

pub fn service_accounts_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let action = args.first().map(String::as_str).unwrap_or("list");
    let rest = args.get(1..).unwrap_or(&[]);
    match action {
        "list" => list_accounts(cmd),
        "create" => create_account(cmd),
        "revoke" => revoke_account(rest, cmd),
        "keys" => keys_command(rest, cmd),
        other => usage(
            format!("Unknown command `nexus service-accounts {other}`."),
            Some("Run `nexus service-accounts --help`."),
        ),
    }
}

/// Gateway's words with a capital first letter, unless the first word is a field name (`expires_at`).
fn sentence(message: &str) -> String {
    let first = message.split(' ').next().unwrap_or("");
    if !first.is_empty() && first.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'\'') {
        capitalize(message)
    } else {
        message.to_string()
    }
}

/// A refusal prints Gateway's own words; a role refusal also says who may.
pub(crate) fn asked(result: Result<Value>) -> Result<Value> {
    match result {
        Err(Error::Core(error))
            if matches!(error.status, 400 | 403 | 404 | 409)
                && error.code.as_deref().is_some_and(|code| !code.is_empty()) =>
        {
            let message = sentence(&error.message);
            if error.status == 403 && error.code.as_deref() == Some("role_required") {
                Err(CliError::hint(message, "Ask an owner or admin of your organization.").into())
            } else {
                Err(CliError::new(message).into())
            }
        }
        other => other,
    }
}

fn keys_text(count: f64) -> String {
    format!("{} live key{}", js_number(count), if count == 1.0 { "" } else { "s" })
}

fn lapsed(record: &Value, now: f64) -> bool {
    s(record, "expiresAt").and_then(parse_ms).is_some_and(|at| at <= now)
}

fn account_status(account: &Value, now: f64) -> &'static str {
    if t(account, "revokedAt") {
        "revoked"
    } else if lapsed(account, now) {
        "expired"
    } else {
        "active"
    }
}

fn key_status(key: &Value, now: f64) -> &'static str {
    if t(key, "revokedAt") {
        "revoked"
    } else if lapsed(key, now) {
        "expired"
    } else {
        "live"
    }
}

fn list_accounts(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let answer = asked(cmd.core.get("/v1/service-accounts"))?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    let accounts = a(&answer, "serviceAccounts");
    if accounts.is_empty() {
        cmd.out(format!(
            "No service accounts yet. Create one: {}",
            style.cyan("nexus service-accounts create --name ci --team <slug> --expires 90d")
        ));
        return Ok(0);
    }
    let now = now_ms();
    let rows = accounts
        .iter()
        .map(|account| {
            let live = a(account, "keys")
                .iter()
                .filter(|key| key_status(key, now) == "live")
                .count();
            let status = account_status(account, now);
            Row {
                cells: vec![
                    js_string(account.get("id")),
                    js_string(account.get("name")),
                    js_string(account.get("teamId")),
                    js_string(account.get("ownerUserId")),
                    live.to_string(),
                    day(s(account, "expiresAt")),
                    status.to_string(),
                ],
                muted: status != "active",
                ..Row::default()
            }
        })
        .collect();
    cmd.lines(list(
        style,
        &[
            col("Account"),
            flex("Name"),
            col("Team"),
            col("Owner"),
            right("Keys"),
            col("Expires"),
            col("Status"),
        ],
        rows,
        None,
    ));
    let keys: Vec<&Value> = accounts.iter().flat_map(|account| a(account, "keys")).collect();
    if !keys.is_empty() {
        let rows = keys
            .iter()
            .map(|key| {
                let status = key_status(key, now);
                Row {
                    cells: vec![
                        js_string(key.get("id")),
                        js_string(key.get("serviceAccountId")),
                        format!("{}…", js_string(key.get("prefix"))),
                        s(key, "label").map(String::from).unwrap_or_else(|| style.dim("–")),
                        js_string(key.get("ownerUserId")),
                        js_string(key.get("scope").and_then(|scope| scope.get("teamId"))),
                        day(s(key, "expiresAt")),
                        s_nonempty(key, "lastUsedAt")
                            .map(|at_time| ago_at(at_time, now))
                            .unwrap_or_else(|| style.dim("never")),
                        status.to_string(),
                    ],
                    muted: status != "live",
                    ..Row::default()
                }
            })
            .collect();
        cmd.out("");
        cmd.lines(list(
            style,
            &[
                col("Key"),
                col("Account"),
                col("Prefix"),
                flex("Label"),
                col("Owner"),
                col("Team"),
                col("Expires"),
                col("Last used"),
                col("Status"),
            ],
            rows,
            None,
        ));
    }
    cmd.out("");
    cmd.out(style.dim("Issue a key: nexus service-accounts keys create <account id> --label <label>"));
    Ok(0)
}

fn create_account(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let (Some(name), Some(team), Some(expires)) = (
        cmd.args.flag_set("name"),
        cmd.args.flag_set("team"),
        cmd.args.flag_set("expires"),
    ) else {
        return usage(CREATE_USAGE, Some("Run `nexus team list` to see the team slugs."));
    };
    if cmd.has("owner") {
        return usage(
            "--owner needs a user id.",
            Some("Leave it out to make yourself the owner."),
        );
    }
    let expires_at = account_expiry(Some(expires), now_ms())?;
    let dry_run = cmd.has("dry-run");
    let body = obj(vec![
        ("name", Some(json!(name))),
        ("team", Some(json!(team))),
        ("expiresAt", Some(json!(expires_at))),
        ("ownerUserId", cmd.args.flag_set("owner").map(|owner| json!(owner))),
        ("dryRun", dry_run.then(|| json!(true))),
    ]);
    let created = asked(cmd.core.post("/v1/service-accounts", body))?;
    if cmd.json {
        return Ok(cmd.print(&created));
    }
    let (id, owner, expires_at) = (
        js_string(created.get("id")),
        js_string(created.get("ownerUserId")),
        js_string(created.get("expiresAt")),
    );
    if t(&created, "dryRun") {
        cmd.out(format!(
            "{} Would create service account {} for team {team}: owner {owner}, expires {expires_at}. \
             Run without --dry-run to create it.",
            style.warn,
            style.bold(&js_string(created.get("name")))
        ));
        return Ok(0);
    }
    cmd.out(format!(
        "{} Created service account {}",
        style.ok,
        style.bold(&js_string(created.get("name")))
    ));
    cmd.lines(pairs(
        style,
        &[
            kv("Id", id.clone()),
            kv("Team", team),
            kv("Owner", owner),
            kv("Expires", expires_at),
        ],
    ));
    cmd.out(style.dim(&format!(
        "Issue its first key: nexus service-accounts keys create {id} --label <label>"
    )));
    Ok(0)
}

fn revoke_account(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let Some(id) = args.first().filter(|id| !id.is_empty()) else {
        return usage("Usage: nexus service-accounts revoke <id> [--dry-run]", Some(LIST_HINT));
    };
    let dry_run = cmd.has("dry-run");
    let path = format!(
        "/v1/service-accounts/{}{}",
        enc(id),
        if dry_run { "?dryRun=1" } else { "" }
    );
    let revoked = asked(cmd.core.delete(&path))?;
    if cmd.json {
        return Ok(cmd.print(&revoked));
    }
    let revoked_id = js_string(revoked.get("id"));
    let count = revoked.get("keysRevoked").and_then(Value::as_f64).unwrap_or(0.0);
    cmd.out(if t(&revoked, "already") {
        format!("{} {revoked_id} was already revoked.", style.ok)
    } else if t(&revoked, "dryRun") {
        format!(
            "{} Would revoke {revoked_id}: {}. Run without --dry-run to revoke it.",
            style.warn,
            if count != 0.0 {
                format!("{} would stop working at once", keys_text(count))
            } else {
                "it has no live keys".to_string()
            }
        )
    } else {
        format!(
            "{} Revoked {revoked_id}: {}.",
            style.ok,
            if count != 0.0 {
                format!("{} stopped", keys_text(count))
            } else {
                "it had no live keys".to_string()
            }
        )
    });
    Ok(0)
}

/// The token is in this answer only: the human output prints it once, with the warning.
fn key_lines(style: &Style, heading: String, key: &Value) -> Vec<String> {
    let mut lines = vec![heading];
    lines.extend(pairs(
        style,
        &[
            kv("Key", js_string(key.get("id"))),
            kv("Label", s(key, "label").unwrap_or("–")),
            kv("Owner", js_string(key.get("ownerUserId"))),
            kv(
                "Team",
                js_string(key.get("scope").and_then(|scope| scope.get("teamId"))),
            ),
            kv("Expires", js_string(key.get("expiresAt"))),
            kv("Token", js_string(key.get("token"))),
        ],
    ));
    lines.push(format!("{} {TOKEN_WARNING}", style.warn));
    lines
}

fn key_path(id: &str, tail: &[&str]) -> String {
    let mut path = format!("/v1/service-accounts/{}", enc(id));
    for part in tail {
        path.push('/');
        path.push_str(&enc(part));
    }
    path
}

fn keys_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let word = |at: usize| args.get(at).map(String::as_str).filter(|word| !word.is_empty());
    let (action, id, key_id) = (word(0), word(1), word(2));
    if matches!(action, Some("create" | "rotate" | "revoke")) && cmd.has("dry-run") {
        return usage(
            "--dry-run does not apply to keys: the API has no dry run for them, so the key would change for real.",
            Some("Use --dry-run with `nexus service-accounts create` or `nexus service-accounts revoke <id>`."),
        );
    }
    match action {
        Some("create") => {
            let (Some(id), Some(label)) = (id, cmd.args.flag_set("label")) else {
                return usage(KEY_CREATE_USAGE, Some(LIST_HINT));
            };
            let body = obj(vec![
                ("label", Some(json!(label))),
                ("expiresAt", key_expiry(cmd)?.map(|expires_at| json!(expires_at))),
            ]);
            let issued = asked(cmd.core.post(&key_path(id, &["keys"]), body))?;
            if cmd.json {
                return Ok(cmd.print(&issued));
            }
            cmd.lines(key_lines(style, format!("{} Issued a key for {id}", style.ok), &issued));
            Ok(0)
        }
        Some("rotate") => {
            let (Some(id), Some(key_id)) = (id, key_id) else {
                return usage(ROTATE_USAGE, Some(LIST_HINT));
            };
            let label = cmd.args.flag_set("label");
            if label.is_none() && (cmd.flag("label").is_some() || cmd.has("label")) {
                return usage("--label needs a name.", Some(ROTATE_USAGE));
            }
            let body = obj(vec![
                ("label", label.map(|label| json!(label))),
                ("expiresAt", key_expiry(cmd)?.map(|expires_at| json!(expires_at))),
            ]);
            let rotated = asked(cmd.core.post(&key_path(id, &["keys", key_id, "rotate"]), body))?;
            if cmd.json {
                return Ok(cmd.print(&rotated));
            }
            cmd.lines(key_lines(
                style,
                format!("{} Rotated key {key_id} of {id}: the old key no longer works", style.ok),
                &rotated,
            ));
            Ok(0)
        }
        Some("revoke") => {
            let (Some(id), Some(key_id)) = (id, key_id) else {
                return usage(
                    "Usage: nexus service-accounts keys revoke <account id> <key id>",
                    Some(LIST_HINT),
                );
            };
            let revoked = asked(cmd.core.delete(&key_path(id, &["keys", key_id])))?;
            if cmd.json {
                return Ok(cmd.print(&revoked));
            }
            cmd.out(format!(
                "{} Key {} of {id} is revoked: it no longer works.",
                style.ok,
                js_string(revoked.get("id"))
            ));
            Ok(0)
        }
        _ => usage(
            "Usage: nexus service-accounts keys <create|rotate|revoke> <account id> …",
            Some(LIST_HINT),
        ),
    }
}
