//! `nexus org` (`org.ts`): your organizations and the one you are signed in to. One noun, like `nexus team`;
//! `nexus orgs` is the same command (a hidden alias).
//!
//! - `nexus org [list]` and `nexus org switch <org>` (Rust gateway): the organizations you belong to, your personal
//!   organization first, and signing in to another of them without a new device login. Switching asks Gateway for a
//!   token of the other organization (`POST /v1/orgs/switch`), saves it with that organization for Nexus UI's links,
//!   forgets the chat session (it belongs to the other organization) and revokes the CLI token it replaces (an API key
//!   is only forgotten, as on logout).
//! - `nexus org settings [list|get|set]`: the organization's settings (typed settings), read by everyone in it and
//!   changed by owners and admins: how long chat messages are kept, whether subscription mode may run, and how Nexus
//!   Artifacts may be shared outside the organization.
//! - `nexus org domains [list|add|remove]` (Rust gateway): the verified email domains, kept by the sign-in provider.
//!   Everyone with a verified email on one joins as its enrollment mode says. Owners and admins add only a domain they
//!   have a verified email on; `--dry-run` sends nothing.
//! - `nexus org subscription [show|set]` (Rust gateway): the plan, status, payer, billing contact and paid term. Everyone
//!   reads it; owners change it, and Billing alone sets its status. A user is a member's id or email, `none` clears a
//!   field, and `--dry-run` writes nothing.

use serde_json::{Map, Value, json};

use crate::artifacts::value_text;
use crate::catalog::ORG_COMMANDS;
use crate::cli::{Cmd, cli_client};
use crate::config::{load_config, save_config};
use crate::error::{CliError, Error, Result, usage};
use crate::json::{a, js_string, s, t};
use crate::style::{Row, Style, col, flex, kv, list, meta, pad_end, pairs, right, rows_of};

/// How people with a verified email on a domain join; the first is the default.
pub const ENROLLMENT_MODES: &[&str] = &["automatic_invitation", "automatic_suggestion", "manual_invitation"];

/// An enrollment mode in words.
pub fn enrollment_text(mode: &str) -> &str {
    match mode {
        "automatic_invitation" => "invited automatically",
        "automatic_suggestion" => "suggested; an admin accepts each",
        "manual_invitation" => "invitation only",
        "enterprise_sso" => "through SSO",
        other => other,
    }
}

/// The settings in the order they print; any other key Gateway answers with comes after them.
pub const SETTING_KEYS: &[&str] = &[
    "chatRetentionDays",
    "agentSubscriptions",
    "growthEvents",
    "chatSuggestions",
    "sessionContent",
    "webSessionViewer",
    "artifacts.external",
    "artifacts.external.code",
    "artifacts.external.domains",
    "artifacts.external.expiry",
    "artifacts.external.continue",
    "artifacts.external.who",
    "artifacts.pages",
    "artifacts.pages.public",
    "artifacts.pages.queries",
    "webSearch.engine",
    "webSearch.model",
    "webSearch.domains",
    "webSearch.maxPerTurn",
    "webSearch.requireZdr",
];

fn org_waiting_enabled() -> bool {
    std::env::var("NEXUS_ORG_WAITING").is_ok_and(|value| value.trim() == "on")
}

pub fn org_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus org <command>", style.bold("Usage")),
        String::new(),
        "You have a personal organization (just you) and any company organizations you belong to.".to_string(),
        "Settings: how long chat messages are kept, whether `nexus claude --subscription` may run, whether growth events"
            .to_string(),
        "are recorded, how artifacts may be shared outside the organization, and whether web search runs;".to_string(),
        "everyone in it reads them, owners and admins change them.".to_string(),
        String::new(),
        style.bold("Commands"),
    ];
    lines.extend(
        ORG_COMMANDS
            .iter()
            .filter(|(name, _)| *name != "waiting" || org_waiting_enabled())
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(&format!("org {name}"), 32)))),
    );
    lines
}

/// The command names of `nexus org help --json`, once each.
pub fn org_command_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for (name, _) in ORG_COMMANDS {
        let first = name.split(' ').next().unwrap_or("");
        if (first != "waiting" || org_waiting_enabled()) && !names.contains(&first) {
            names.push(first);
        }
    }
    names
}

/// "Personal" or "Company".
pub fn kind_text(kind: Option<&str>) -> &'static str {
    if kind == Some("personal") {
        "Personal"
    } else {
        "Company"
    }
}

/// The settings' keys as Gateway answers them: the known ones in order, then any others.
pub fn setting_keys(settings: &Value) -> Vec<String> {
    let empty = Map::new();
    let map = settings.as_object().unwrap_or(&empty);
    let mut keys: Vec<String> = SETTING_KEYS
        .iter()
        .filter(|key| map.contains_key(**key))
        .map(|key| key.to_string())
        .collect();
    keys.extend(
        map.keys()
            .filter(|key| *key != "defaults" && *key != "updatedAt" && !SETTING_KEYS.contains(&key.as_str()))
            .cloned(),
    );
    keys
}

/// A value from the command line: JSON when it parses (`true`, `30`, `null`, `{"allow":[…]}`), else the word itself.
pub fn parse_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

fn default_of<'a>(settings: &'a Value, key: &str) -> Option<&'a Value> {
    settings.get("defaults").and_then(|defaults| defaults.get(key))
}

pub fn org_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    if action == "help" || args.get(1).map(String::as_str) == Some("help") || cmd.has("help") {
        if cmd.json {
            return Ok(cmd.print(&json!({ "usage": "nexus org <command>", "commands": org_command_names() })));
        }
        cmd.lines(org_help(style));
        return Ok(0);
    }
    match action {
        "list" => {
            let answer = cmd.core.get("/v1/orgs")?;
            if cmd.json {
                return Ok(cmd.print(&json!({ "orgs": answer.get("orgs").cloned().unwrap_or(json!([])) })));
            }
            // The one you are signed in to is the selected row (`❯`, in the accent).
            let rows: Vec<Row> = a(&answer, "orgs")
                .iter()
                .map(|org| Row {
                    cells: vec![
                        s(org, "slug")
                            .map(String::from)
                            .unwrap_or_else(|| js_string(org.get("id"))),
                        js_string(org.get("name")),
                        kind_text(s(org, "kind")).into(),
                        s(org, "role").unwrap_or("").into(),
                    ],
                    selected: t(org, "current"),
                    ..Row::default()
                })
                .collect();
            cmd.lines(list(
                style,
                &[col("Organization"), flex("Name"), col("Kind"), col("Role")],
                rows,
                None,
            ));
            Ok(0)
        }
        "waiting" if org_waiting_enabled() => {
            let answer = cmd.core.get("/v1/orgs/waiting")?;
            if cmd.json {
                return Ok(cmd.print(&answer));
            }
            let memberships = cmd.core.get("/v1/orgs")?;
            let rows = a(&memberships, "orgs")
                .iter()
                .map(|org| {
                    let count = s(org, "id")
                        .and_then(|id| answer.get("counts")?.get(id)?.as_i64())
                        .map_or_else(|| "—".into(), |count| count.to_string());
                    vec![js_string(org.get("name")), count]
                })
                .collect();
            cmd.lines(list(
                style,
                &[flex("Organization"), right("Waiting")],
                rows_of(rows),
                None,
            ));
            Ok(0)
        }
        "switch" => {
            let Some(wanted) = args.get(1).filter(|org| !org.is_empty()) else {
                return usage(
                    "Usage: nexus org switch <org>",
                    Some("`nexus org` lists your organizations; `personal` is yours alone."),
                );
            };
            let switched = cmd.core.post("/v1/orgs/switch", json!({ "org": wanted }))?;
            let revoked_previous = sign_in(cmd, &switched)?;
            let tenant = switched.get("tenant").cloned().unwrap_or(Value::Null);
            if cmd.json {
                let mut answer = serde_json::Map::new();
                answer.insert("tenant".into(), tenant);
                for key in ["role", "tokenKind", "expiresAt"] {
                    if let Some(value) = switched.get(key) {
                        answer.insert(key.into(), value.clone());
                    }
                }
                answer.insert("revokedPrevious".into(), json!(revoked_previous));
                return Ok(cmd.print(&Value::Object(answer)));
            }
            let name = js_string(tenant.get("name"));
            cmd.out(format!(
                "{} Signed in to {}  {}",
                style.ok,
                style.bold(s(&tenant, "slug").unwrap_or(&name)),
                meta(
                    style,
                    &[
                        Some(name.clone()),
                        Some(kind_text(s(&tenant, "kind")).to_lowercase()),
                        s(&switched, "role").map(String::from),
                    ]
                )
            ));
            Ok(0)
        }
        "settings" => settings_command(&args[1..], cmd),
        "domains" => domains_command(&args[1..], cmd),
        "subscription" => subscription_command(&args[1..], cmd),
        other => usage(
            format!("Unknown command `nexus org {other}`."),
            Some(
                "Use `nexus org`, `nexus org switch <org>`, `nexus org settings`, `nexus org domains` or `nexus org subscription`.",
            ),
        ),
    }
}

/// `subscription set`'s flags, the fields they change and those fields in words. Billing writes the rest.
const SUBSCRIPTION_FIELDS: &[(&str, &str, &str)] = &[
    ("payer", "payerUserId", "the payer"),
    ("billing-contact", "billingContactUserId", "the billing contact"),
];

const SUBSCRIPTION_SET_USAGE: &str =
    "Usage: nexus org subscription set [--payer U] [--billing-contact U] [--effective-at T] [--dry-run]";

/// A subscription status in words: `past_due` is "past due".
pub fn status_text(status: &str) -> String {
    status.replacen('_', " ", 1)
}

fn subscription_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("show");
    let members = || -> Result<Vec<Value>> { Ok(a(&cmd.core.get("/v1/orgs/members")?, "members").to_vec()) };
    match action {
        "show" => {
            let subscription = cmd.core.get("/v1/orgs/subscription")?;
            if cmd.json {
                return Ok(cmd.print(&subscription));
            }
            let (payer, contact) = (
                s(&subscription, "payerUserId"),
                s(&subscription, "billingContactUserId"),
            );
            let people = if payer.is_some() || contact.is_some() {
                members()?
            } else {
                Vec::new()
            };
            let who = |user_id: Option<&str>| match user_id {
                Some(user_id) => people
                    .iter()
                    .find(|member| s(member, "userId") == Some(user_id))
                    .and_then(|member| s(member, "email"))
                    .unwrap_or(user_id)
                    .to_string(),
                None => style.dim("–"),
            };
            let term = match s(&subscription, "periodStart") {
                Some(start) => format!("{start} to {}", s(&subscription, "periodEnd").unwrap_or("open")),
                None => style.dim("–"),
            };
            let last_change = if s(&subscription, "updatedAt").is_some() {
                format!("effective {}", js_string(subscription.get("effectiveAt")))
            } else {
                style.dim("none yet")
            };
            cmd.lines(pairs(
                style,
                &[
                    kv(
                        "Plan",
                        s(&subscription, "plan")
                            .map(String::from)
                            .unwrap_or_else(|| style.dim("–")),
                    ),
                    kv(
                        "Status",
                        format!(
                            "{} since {}",
                            status_text(s(&subscription, "status").unwrap_or("")),
                            js_string(subscription.get("statusEffectiveAt"))
                        ),
                    ),
                    kv("Payer", who(payer)),
                    kv("Billing contact", who(contact)),
                    kv("Term", term),
                    kv(
                        "Seats",
                        subscription
                            .get("seats")
                            .and_then(Value::as_u64)
                            .map_or_else(|| style.dim("–"), |seats| seats.to_string()),
                    ),
                    kv("Last change", last_change),
                ],
            ));
            Ok(0)
        }
        "set" => {
            let mut body = Map::new();
            let mut people: Option<Vec<Value>> = None;
            for (flag, field, _) in SUBSCRIPTION_FIELDS {
                let Some(value) = cmd.flag(flag).map(str::trim) else {
                    continue;
                };
                if value == "none" {
                    body.insert(field.to_string(), Value::Null);
                } else if value.contains('@') {
                    if people.is_none() {
                        people = Some(members()?);
                    }
                    let wanted = value.to_lowercase();
                    let found = people
                        .iter()
                        .flatten()
                        .find(|member| s(member, "email").is_some_and(|email| email.to_lowercase() == wanted))
                        .and_then(|member| s(member, "userId"));
                    let Some(user_id) = found else {
                        return Err(Error::Cli(CliError::hint(
                            format!("Nobody in this organization has the email {value}."),
                            "`nexus members` lists them.",
                        )));
                    };
                    body.insert(field.to_string(), Value::String(user_id.to_string()));
                } else {
                    body.insert(field.to_string(), Value::String(value.to_string()));
                }
            }
            if body.is_empty() {
                return usage(
                    SUBSCRIPTION_SET_USAGE,
                    Some(
                        "Name the payer or the billing contact; none clears it. A user is a member's id or email. Billing sets the plan, the term and the status.",
                    ),
                );
            }
            if let Some(effective_at) = cmd.flag("effective-at") {
                body.insert("effectiveAt".into(), Value::String(effective_at.to_string()));
            }
            if cmd.has("dry-run") {
                body.insert("dryRun".into(), Value::Bool(true));
            }
            let result = cmd.core.patch("/v1/orgs/subscription", Value::Object(body))?;
            if cmd.json {
                return Ok(cmd.print(&result));
            }
            let changed = a(&result, "changed");
            if changed.is_empty() {
                cmd.out(style.dim("Nothing to change: the subscription already says that."));
                return Ok(0);
            }
            let words: Vec<&str> = changed
                .iter()
                .filter_map(Value::as_str)
                .map(|changed| {
                    SUBSCRIPTION_FIELDS
                        .iter()
                        .find(|(_, field, _)| *field == changed)
                        .map_or(changed, |(_, _, words)| *words)
                })
                .collect();
            let words = words.join(", ");
            if t(&result, "dryRun") {
                cmd.out(format!(
                    "{} would change {words}. Nothing changed.",
                    style.dim("Dry run:")
                ));
            } else {
                cmd.out(format!(
                    "{} Changed {words}, effective {}.",
                    style.ok,
                    js_string(
                        result
                            .get("subscription")
                            .and_then(|subscription| subscription.get("effectiveAt"))
                    )
                ));
            }
            Ok(0)
        }
        other => usage(
            format!("Unknown command `nexus org subscription {other}`."),
            Some("Use show or set."),
        ),
    }
}

fn domains_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    let wanted = args
        .get(1)
        .map(|wanted| wanted.trim())
        .filter(|wanted| !wanted.is_empty());
    let dry_run = cmd.has("dry-run");
    let listed = || -> Result<Vec<Value>> { Ok(a(&cmd.core.get("/v1/orgs/domains")?, "domains").to_vec()) };
    match action {
        "list" => {
            let domains = listed()?;
            if cmd.json {
                return Ok(cmd.print(&json!({ "domains": domains })));
            }
            if domains.is_empty() {
                cmd.out(style.dim(
                    "No email domains yet. Add one you have a verified email on: nexus org domains add <domain>.",
                ));
                return Ok(0);
            }
            let rows: Vec<Vec<String>> = domains
                .iter()
                .map(|domain| {
                    vec![
                        js_string(domain.get("domain")),
                        enrollment_text(s(domain, "enrollmentMode").unwrap_or("")).to_string(),
                        js_string(domain.get("pendingInvitations")),
                        js_string(domain.get("id")),
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[col("Domain"), flex("People join"), right("Pending"), col("Id")],
                rows_of(rows),
                None,
            ));
            Ok(0)
        }
        "add" => {
            let Some(wanted) = wanted else {
                return usage(
                    "Usage: nexus org domains add <domain> [--enrollment M] [--dry-run]",
                    Some("A domain you have a verified email on, as acme.com."),
                );
            };
            let enrollment_mode = cmd.flag("enrollment").unwrap_or(ENROLLMENT_MODES[0]);
            if !ENROLLMENT_MODES.contains(&enrollment_mode) {
                return usage(
                    format!("--enrollment must be one of {}.", ENROLLMENT_MODES.join(", ")),
                    None,
                );
            }
            let domain = wanted.to_lowercase();
            if dry_run {
                let already = listed()?
                    .iter()
                    .any(|existing| s(existing, "domain") == Some(domain.as_str()));
                if cmd.json {
                    return Ok(cmd.print(&json!({
                        "dryRun": true,
                        "domain": domain,
                        "enrollmentMode": enrollment_mode,
                        "alreadyAdded": already,
                    })));
                }
                let what = if already {
                    format!("{} is already added", style.bold(&domain))
                } else {
                    format!(
                        "would add {} ({})",
                        style.bold(&domain),
                        enrollment_text(enrollment_mode)
                    )
                };
                cmd.out(format!("{} {what}. Nothing changed.", style.dim("Dry run:")));
                return Ok(0);
            }
            let added = cmd.core.post(
                "/v1/orgs/domains",
                json!({ "domain": domain, "enrollmentMode": enrollment_mode }),
            )?;
            if cmd.json {
                return Ok(cmd.print(&added));
            }
            cmd.out(format!(
                "{} Added {}: people with a verified email there are {}.",
                style.ok,
                style.bold(&js_string(added.get("domain"))),
                enrollment_text(s(&added, "enrollmentMode").unwrap_or(""))
            ));
            Ok(0)
        }
        "remove" => {
            let Some(wanted) = wanted else {
                return usage(
                    "Usage: nexus org domains remove <domain> [--dry-run]",
                    Some("Its name or id; `nexus org domains` lists them."),
                );
            };
            let name = wanted.to_lowercase();
            let domains = listed()?;
            let Some(target) = domains
                .iter()
                .find(|domain| s(domain, "domain") == Some(name.as_str()) || s(domain, "id") == Some(wanted))
            else {
                return Err(Error::Cli(CliError::hint(
                    format!("This organization has no domain {wanted}."),
                    "`nexus org domains` lists them.",
                )));
            };
            let (domain, id) = (js_string(target.get("domain")), js_string(target.get("id")));
            if dry_run {
                if cmd.json {
                    return Ok(cmd.print(&json!({ "dryRun": true, "domain": domain, "id": id })));
                }
                cmd.out(format!(
                    "{} would remove {}. Nothing changed.",
                    style.dim("Dry run:"),
                    style.bold(&domain)
                ));
                return Ok(0);
            }
            cmd.core.delete(&format!("/v1/orgs/domains/{}", encode_segment(&id)))?;
            if cmd.json {
                return Ok(cmd.print(&json!({ "removed": true, "domain": domain, "id": id })));
            }
            cmd.out(format!(
                "{} Removed {}. The sign-in provider stops inviting people on it; who joined stays.",
                style.ok,
                style.bold(&domain)
            ));
            Ok(0)
        }
        other => usage(
            format!("Unknown command `nexus org domains {other}`."),
            Some("Use list, add or remove."),
        ),
    }
}

/// A path segment as `encodeURIComponent` writes it.
pub(crate) fn encode_segment(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn settings_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    let key = args.get(1);
    let rest = if args.len() > 2 { &args[2..] } else { &[] };
    match action {
        "list" => {
            let settings = cmd.core.get("/v1/orgs/settings")?;
            if cmd.json {
                return Ok(cmd.print(&settings));
            }
            let rows: Vec<Vec<String>> = setting_keys(&settings)
                .iter()
                .map(|name| {
                    vec![
                        name.clone(),
                        value_text(settings.get(name)),
                        value_text(default_of(&settings, name)),
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[col("Setting"), col("Value"), flex("Default")],
                rows_of(rows),
                None,
            ));
            cmd.out(style.dim("Change one: nexus org settings set <key> <value> (owners and admins)."));
            Ok(0)
        }
        "get" => {
            let Some(key) = key else {
                return usage(
                    "Usage: nexus org settings get <key>",
                    Some("`nexus org settings` lists them."),
                );
            };
            let settings = cmd.core.get("/v1/orgs/settings")?;
            if !setting_keys(&settings).contains(key) {
                return usage(
                    format!("Unknown setting {key}."),
                    Some("`nexus org settings` lists them."),
                );
            }
            if cmd.json {
                let value = settings.get(key).cloned().unwrap_or(Value::Null);
                let default = default_of(&settings, key).cloned().unwrap_or(Value::Null);
                return Ok(cmd.print(&json!({ "key": key, "value": value, "default": default })));
            }
            cmd.lines(pairs(
                style,
                &[
                    kv("Setting", key.clone()),
                    kv("Value", value_text(settings.get(key))),
                    kv("Default", value_text(default_of(&settings, key))),
                ],
            ));
            Ok(0)
        }
        "set" => {
            let Some(key) = key.filter(|_| !rest.is_empty()) else {
                return usage(
                    "Usage: nexus org settings set <key> <value>",
                    Some("A JSON value or a word; null restores the default."),
                );
            };
            let mut body = Map::new();
            body.insert(key.clone(), parse_value(&rest.join(" ")));
            let settings = cmd.core.put("/v1/orgs/settings", Value::Object(body))?;
            if cmd.json {
                return Ok(cmd.print(&settings));
            }
            cmd.out(format!("{} {key} is now {}.", style.ok, value_text(settings.get(key))));
            Ok(0)
        }
        other => usage(
            format!("Unknown command `nexus org settings {other}`."),
            Some("Use list, get or set."),
        ),
    }
}

/// Save the new token and its organization; revoke the CLI token it replaces (best effort). Whether it revoked one.
fn sign_in(cmd: &Cmd, switched: &Value) -> Result<bool> {
    let config = cmd.config;
    let revoked = match (&config.api_key, config.kind.as_deref()) {
        (Some(key), kind) if kind != Some("api") => cli_client(&config.url, Some(key))
            .post("/v1/auth/logout", json!({}))
            .map(|answer| t(&answer, "revoked"))
            .unwrap_or(false),
        _ => false,
    };
    let tenant = switched.get("tenant").cloned().unwrap_or(Value::Null);
    let org = s(&tenant, "slug")
        .filter(|slug| !slug.is_empty())
        .map(String::from)
        .unwrap_or_else(|| js_string(tenant.get("id")));
    let mut saved = load_config(&cmd.ctx.config_path, &cmd.ctx.env_fn());
    saved.url = config.url.clone();
    saved.api_key = Some(js_string(switched.get("token")));
    saved.kind = Some("cli".into());
    saved.chat_session = None;
    saved.extra.insert("org".into(), json!(org));
    save_config(&cmd.ctx.config_path, &saved).map_err(|error| {
        Error::Cli(CliError::new(format!(
            "Could not write {}: {error}",
            cmd.ctx.config_path.display()
        )))
    })?;
    Ok(revoked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_parse_as_json_or_words_and_keys_keep_their_order() {
        assert_eq!(parse_value("null"), Value::Null);
        assert_eq!(parse_value("30"), json!(30));
        assert_eq!(parse_value("named"), json!("named"));
        assert_eq!(parse_value("{\"allow\":[\"a.test\"]}"), json!({ "allow": ["a.test"] }));
        assert_eq!(
            setting_keys(&json!({ "extra": 1, "chatRetentionDays": 1, "defaults": {}, "updatedAt": "x" })),
            vec!["chatRetentionDays".to_string(), "extra".to_string()]
        );
        assert_eq!(value_text(Some(&json!({ "allow": [] }))), "{\"allow\":[]}");
        assert_eq!(value_text(Some(&Value::Null)), "–");
        assert_eq!(value_text(Some(&json!(true))), "true");
    }

    #[test]
    fn enrollment_modes_read_as_words_and_ids_stay_one_segment() {
        assert_eq!(enrollment_text("automatic_invitation"), "invited automatically");
        assert_eq!(enrollment_text("manual_invitation"), "invitation only");
        assert_eq!(enrollment_text("something_new"), "something_new");
        assert_eq!(ENROLLMENT_MODES[0], "automatic_invitation");
        assert_eq!(encode_segment("orgdmn_2abc"), "orgdmn_2abc");
        assert_eq!(encode_segment("a/b c"), "a%2Fb%20c");
    }
}
