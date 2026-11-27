//! Models and model access policies (`models.ts`): `nexus models`, `nexus policy show|set|test`. A deny
//! always wins, and each level only narrows the ones above it. Also `nexus policy connectors|connector`: which
//! connectors the organization's members may use.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use nexus_client::agent_entries::Agent;

use crate::catalog::{POLICY_VERBS, policy_commands};
use crate::cli::Cmd;
use crate::client::{enc, search};
use crate::config::{load_config, save_config};
use crate::error::{CliError, Error, Result, usage};
use crate::json::{a, js_string, n, obj, s, t};
use crate::picker::{PickItem, PickerSpec, run_picker};
use crate::style::{
    Group, Row, Style, capitalize, col, flex, grouped_list, iso, kv, legend, meter, now_ms, pad_end, pairs,
    provider_mark, provider_name, provider_of, quote, right, usd,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub level: &'static str,
    pub subject: Option<String>,
    pub label: String,
}

/// The agents a policy applies to: those with a Gateway route. Gateway refuses a policy for any other
/// (`route_unsupported`) and names no default for it.
pub(crate) fn policy_agents() -> impl Iterator<Item = Agent> {
    Agent::ALL
        .into_iter()
        .filter(|agent| agent.entry().billing.gateway.is_some())
}

/// [`policy_agents`]' ids, joined by `sep`.
pub(crate) fn policy_agent_ids(sep: &str) -> String {
    policy_agents().map(Agent::as_str).collect::<Vec<_>>().join(sep)
}

/// What a policy's defaults name a model for: chat, then each agent a policy applies to.
fn default_kinds() -> impl Iterator<Item = &'static str> {
    std::iter::once("chat").chain(policy_agents().map(Agent::as_str))
}

/// `org` (or nothing), `team/<slug>`, `agent/<agent>` for an agent a policy applies to ([`policy_agents`]).
pub fn policy_target(value: Option<&str>) -> Result<Target> {
    let value = value.filter(|v| !v.is_empty());
    let Some(value) = value.filter(|v| *v != "org" && *v != "organization") else {
        return Ok(Target {
            level: "organization",
            subject: None,
            label: "the organization".into(),
        });
    };
    let parsed = ["team/", "agent/"].iter().find_map(|prefix| {
        value
            .strip_prefix(prefix)
            .filter(|rest| !rest.is_empty())
            .map(|rest| (*prefix, rest))
    });
    let Some((prefix, subject)) = parsed else {
        return usage(
            format!("Unknown policy target {}.", quote(value)),
            Some(&format!(
                "Name it as org, team/<slug> or agent/<{}>.",
                policy_agent_ids("|")
            )),
        );
    };
    let level = if prefix == "team/" { "team" } else { "agent" };
    if level == "agent" && !policy_agents().any(|agent| agent.as_str() == subject) {
        return usage(
            format!("No agent {subject}."),
            Some(&format!("Agents: {}.", policy_agent_ids(", "))),
        );
    }
    Ok(Target {
        level,
        subject: Some(subject.to_string()),
        label: format!("{level} {subject}"),
    })
}

fn policy_path(target: &Target) -> String {
    match target.level {
        "organization" => "/v1/policies/organization".into(),
        level => format!(
            "/v1/policies/{}/{}",
            if level == "team" { "teams" } else { "agents" },
            enc(target.subject.as_deref().unwrap_or(""))
        ),
    }
}

/// Who an operator control is on: `org`, `team/<slug>`, `person/<id>` or `key/<id>`.
fn control_target(value: Option<&str>) -> Result<Target> {
    let value = value.unwrap_or("");
    if value == "org" || value == "organization" {
        return Ok(Target {
            level: "organization",
            subject: None,
            label: "the organization".into(),
        });
    }
    let parsed = ["team", "person", "key"].into_iter().find_map(|level| {
        value
            .strip_prefix(level)
            .and_then(|rest| rest.strip_prefix('/'))
            .filter(|subject| !subject.is_empty())
            .map(|subject| (level, subject))
    });
    let Some((level, subject)) = parsed else {
        return usage(
            format!("Unknown control target {}.", quote(value)),
            Some("Name it as org, team/<slug>, person/<id> or key/<id>."),
        );
    };
    Ok(Target {
        level,
        subject: Some(subject.to_string()),
        label: format!("{level} {subject}"),
    })
}

/// `12h`, `30d` or a number of days, as the ISO time that far after `now`; None for anything else.
pub fn duration_expiry(value: &str, now: f64) -> Option<String> {
    let (number, hours_per_unit) = match (value.strip_suffix('h'), value.strip_suffix('d')) {
        (Some(number), _) => (number, 1),
        (_, Some(number)) => (number, 24),
        _ => (value, 24),
    };
    Some(number)
        .filter(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|number| number.parse::<u64>().ok())
        .and_then(|number| number.checked_mul(hours_per_unit))
        .filter(|hours| *hours > 0 && *hours <= 24 * 3650)
        .map(|hours| iso(now + hours as f64 * 3_600_000.0))
}

/// `--expires 12h`, `30d` or a number of days, as the ISO time that far after `now`.
fn control_expiry(value: Option<&str>, now: f64) -> Result<String> {
    match duration_expiry(value.unwrap_or(""), now) {
        Some(expires_at) => Ok(expires_at),
        None => usage("--expires needs a duration: 30d, 12h or a number of days.", None),
    }
}

/// A 403 on a policy write: only an owner or an admin may.
fn owner_or_admin(result: Result<Value>) -> Result<Value> {
    match result {
        Err(Error::Core(error)) if error.status == 403 => Err(CliError::hint(
            capitalize(&error.message),
            "Ask an owner or admin of your organization.",
        )
        .into()),
        other => other,
    }
}

fn control_line(control: &Value) -> String {
    let models = a(control, "models")
        .iter()
        .map(|model| js_string(Some(model)))
        .collect::<Vec<_>>()
        .join(",");
    let who = match js_string(control.get("subjectLevel")).as_str() {
        "organization" => "org".to_string(),
        level => format!("{level}/{}", js_string(control.get("subject"))),
    };
    format!(
        "{}  {}  {}{}  until {}  {}",
        js_string(control.get("id")),
        pad_end(&js_string(control.get("kind")), 8),
        who,
        if models.is_empty() {
            String::new()
        } else {
            format!("  {models}")
        },
        js_string(control.get("expiresAt")),
        js_string(control.get("reason")),
    )
}

fn split_list(value: &str) -> Vec<Value> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| json!(item))
        .collect()
}

/// The rule `policy set` saves: `--from` replaces it whole; otherwise each flag replaces its own part of the current rule.
pub fn rule_from_flags(
    current: &Value,
    flags: &HashMap<String, String>,
    clear: bool,
    read_file: &dyn Fn(&str) -> std::io::Result<String>,
) -> Result<Value> {
    if clear {
        return Ok(json!({}));
    }
    if let Some(from) = flags.get("from").filter(|from| !from.is_empty()) {
        let text =
            read_file(from).map_err(|error| Error::Cli(CliError::new(format!("ENOENT: {error}, open '{from}'"))))?;
        let parsed: Value = yaml_serde::from_str(&text).map_err(|error| {
            Error::Cli(CliError::new(
                error.to_string().lines().next().unwrap_or("").to_string(),
            ))
        })?;
        if !parsed.is_object() {
            return usage(
                format!("{from} is not a rule."),
                Some(&format!(
                    "Write it as {{ allow: [...], deny: [...], defaults: {{ chat, {} }} }} in YAML or JSON.",
                    policy_agent_ids(", ")
                )),
            );
        }
        return Ok(parsed);
    }
    let mut rule: Map<String, Value> = current.as_object().cloned().unwrap_or_default();
    if let Some(allow) = flags.get("allow") {
        let list = if allow == "all" { vec![] } else { split_list(allow) };
        if list.is_empty() {
            rule.remove("allow");
        } else {
            rule.insert("allow".into(), Value::Array(list));
        }
    }
    if let Some(deny) = flags.get("deny") {
        let list = if deny == "none" { vec![] } else { split_list(deny) };
        if list.is_empty() {
            rule.remove("deny");
        } else {
            rule.insert("deny".into(), Value::Array(list));
        }
    }
    let mut defaults: Map<String, Value> = rule
        .get("defaults")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for kind in default_kinds() {
        if let Some(value) = flags.get(&format!("default-{kind}")) {
            if value.is_empty() {
                defaults.remove(kind);
            } else {
                defaults.insert(kind.into(), json!(value));
            }
        }
    }
    defaults.retain(|_, value| crate::json::truthy(value));
    if defaults.is_empty() {
        rule.remove("defaults");
    } else {
        rule.insert("defaults".into(), Value::Object(defaults));
    }
    Ok(Value::Object(rule))
}

fn rule_text(rule: &Value) -> String {
    let level = s(rule, "level").unwrap_or("");
    match level {
        "none" => "no rule restricts it".into(),
        "catalog" => "the model catalog".into(),
        _ => {
            let place = match level {
                "organization" => "organization".to_string(),
                "deployment" => "this Gateway".to_string(),
                other => format!("{other} {}", s(rule, "subject").unwrap_or("")),
            };
            format!(
                "{place}{}{}",
                s(rule, "list")
                    .filter(|l| !l.is_empty())
                    .map(|l| format!(" {l} list"))
                    .unwrap_or_default(),
                s(rule, "pattern")
                    .filter(|p| !p.is_empty())
                    .map(|p| format!(" \"{p}\""))
                    .unwrap_or_default()
            )
        }
    }
}

fn joined(values: &[Value]) -> String {
    values.iter().map(|v| js_string(Some(v))).collect::<Vec<_>>().join(", ")
}

fn policy_lines(style: &Style, view: &Value, label: &str) -> Vec<String> {
    let rule = view.get("rule").cloned().unwrap_or(Value::Null);
    let defaults = rule
        .get("defaults")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(kind, model)| format!("{kind}: {}", js_string(Some(model))))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let mut lines = vec![
        format!(
            "{}{}{}",
            style.bold(&format!("Model policy · {label}")),
            if t(view, "custom") {
                String::new()
            } else {
                style.dim(" (no rule of its own)")
            },
            if t(view, "migrated") {
                style.dim(" (from the agent policy)")
            } else {
                String::new()
            }
        ),
        String::new(),
    ];
    lines.extend(pairs(
        style,
        &[
            kv(
                "Allow",
                if a(&rule, "allow").is_empty() {
                    style.dim("everything the levels above allow")
                } else {
                    joined(a(&rule, "allow"))
                },
            ),
            kv(
                "Deny",
                if a(&rule, "deny").is_empty() {
                    style.dim("nothing")
                } else {
                    joined(a(&rule, "deny"))
                },
            ),
            kv(
                "Defaults",
                if defaults.is_empty() {
                    style.dim("inherited")
                } else {
                    defaults
                },
            ),
        ],
    ));
    let inherited = a(view, "inherited");
    if !inherited.is_empty() {
        lines.push(String::new());
        lines.push(style.dim("Inherited (a deny anywhere wins; every allow list must match)"));
        for entry in inherited {
            let rule = entry.get("rule").cloned().unwrap_or(Value::Null);
            let parts: Vec<String> = [
                if a(&rule, "allow").is_empty() {
                    String::new()
                } else {
                    format!("allow {}", joined(a(&rule, "allow")))
                },
                if a(&rule, "deny").is_empty() {
                    String::new()
                } else {
                    format!("deny {}", joined(a(&rule, "deny")))
                },
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
            let level = js_string(entry.get("level"));
            lines.push(format!(
                "  {}: {}",
                if level == "team" {
                    format!("team {}", js_string(entry.get("subject")))
                } else {
                    level
                },
                if parts.is_empty() {
                    style.dim("defaults only")
                } else {
                    parts.join("; ")
                }
            ));
        }
    }
    lines.push(String::new());
    let effective = view.get("effective").cloned().unwrap_or(Value::Null);
    let allowed = joined(a(&effective, "allowed"));
    let mut entries = vec![kv(
        "Allowed here",
        if allowed.is_empty() {
            style.red("no supported model")
        } else {
            allowed
        },
    )];
    for kind in default_kinds() {
        let entry = effective
            .pointer(&format!("/defaults/{kind}"))
            .cloned()
            .unwrap_or(Value::Null);
        let text = match s(&entry, "model").filter(|m| !m.is_empty()) {
            Some(model) => {
                let from = entry.get("from").cloned().unwrap_or(Value::Null);
                let source = if s(&from, "level") == Some("catalog") {
                    "Nexus's default".to_string()
                } else {
                    rule_text(&from)
                };
                format!(
                    "{model} {}{}",
                    style.dim(&format!("({source})")),
                    s(&entry, "note")
                        .filter(|n| !n.is_empty())
                        .map(|n| style.dim(&format!("; {n}")))
                        .unwrap_or_default()
                )
            }
            None => style.red("nothing allowed"),
        };
        entries.push(kv(&format!("nexus/auto ({kind})"), text));
    }
    lines.extend(pairs(style, &entries));
    for note in a(view, "notes") {
        lines.push(format!("{} {}", style.warn, js_string(Some(note))));
    }
    lines
}

pub fn test_lines(style: &Style, result: &Value) -> Vec<String> {
    let as_ = result.get("as").cloned().unwrap_or(Value::Null);
    let who = s(&as_, "email")
        .or_else(|| s(&as_, "userId"))
        .or_else(|| s(&as_, "keyId"))
        .unwrap_or("you")
        .to_string();
    let head = if t(result, "allowed") {
        format!(
            "{} Allowed: {}{}",
            style.ok,
            style.bold(s(result, "model").unwrap_or("")),
            s(result, "resolvedFrom")
                .filter(|r| !r.is_empty())
                .map(|r| style.dim(&format!(" ({r})")))
                .unwrap_or_default()
        )
    } else {
        format!(
            "{} Refused: {} {}",
            style.fail,
            style.bold(
                &s(result, "model")
                    .map(String::from)
                    .unwrap_or_else(|| js_string(result.get("requested")))
            ),
            style.dim(&format!("({})", s(result, "code").unwrap_or("model_not_allowed")))
        )
    };
    let mut lines = vec![
        head,
        format!("  {}", style.dim(&js_string(result.get("reason")))),
        String::new(),
    ];
    let allowed = joined(a(result, "allowedModels"));
    lines.extend(pairs(
        style,
        &[
            kv(
                "As",
                format!(
                    "{who}{}{}",
                    s(result, "team")
                        .filter(|t| !t.is_empty())
                        .map(|t| format!(" in team {t}"))
                        .unwrap_or_default(),
                    s(result, "agent")
                        .filter(|a| !a.is_empty())
                        .map(|a| format!(" with {a}"))
                        .unwrap_or_default()
                ),
            ),
            kv("API", js_string(result.get("surface"))),
            kv("Rule", rule_text(result.get("rule").unwrap_or(&Value::Null))),
            kv(
                "Allowed here",
                if allowed.is_empty() {
                    style.red("no supported model")
                } else {
                    allowed
                },
            ),
        ],
    ));
    let warnings = a(result, "warnings");
    for note in a(result, "notes").iter().filter(|note| !warnings.contains(note)) {
        lines.push(format!("{} {}", style.info, js_string(Some(note))));
    }
    for warning in warnings {
        lines.push(format!("{} {}", style.warn, js_string(Some(warning))));
    }
    lines
}

pub fn policy_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("show");
    let who = args.get(1).map(String::as_str);
    match action {
        "show" => {
            let target = policy_target(who)?;
            let view = cmd.core.get(&policy_path(&target))?;
            if cmd.json {
                return Ok(cmd.print(&view));
            }
            cmd.lines(policy_lines(style, &view, &target.label));
            Ok(0)
        }
        "set" => {
            let target = policy_target(who)?;
            if who.is_none_or(str::is_empty) {
                return usage(
                    "Usage: nexus policy set <org|team/<slug>|agent/<name>> [--allow a,b] [--deny c] [--default m] [--from rule.yaml] [--clear]",
                    None,
                );
            }
            let current = cmd.core.get(&policy_path(&target))?;
            // `--default m`: the agent's default for an agent, the chat default (nexus/auto) otherwise.
            let mut flags = cmd.args.flags.clone();
            if let Some(default) = cmd.flag("default") {
                let kind = if target.level == "agent" {
                    target.subject.clone().unwrap_or_default()
                } else {
                    "chat".into()
                };
                flags.insert(format!("default-{kind}"), default.to_string());
            }
            let base = if t(&current, "custom") {
                current.get("rule").cloned().unwrap_or(json!({}))
            } else {
                json!({})
            };
            let rule = rule_from_flags(&base, &flags, cmd.has("clear"), &|path| cmd.read_file(path))?;
            let saved = owner_or_admin(cmd.core.put(&policy_path(&target), rule))?;
            if cmd.json {
                return Ok(cmd.print(&saved));
            }
            cmd.out(format!("{} Saved the model policy of {}.", style.ok, target.label));
            cmd.out("");
            cmd.lines(policy_lines(style, &saved, &target.label));
            Ok(0)
        }
        "test" => {
            let model = cmd.flag("model").or(who);
            let body = obj(vec![
                ("model", model.map(|m| json!(m))),
                ("as", cmd.args.flag_set("as").map(|v| json!(v))),
                ("team", cmd.args.flag_set("team").map(|v| json!(v))),
                ("agent", cmd.args.flag_set("agent").map(|v| json!(v))),
            ]);
            let result = cmd.core.post("/v1/policies/test", body)?;
            if cmd.json {
                return Ok(cmd.print(&result));
            }
            cmd.lines(test_lines(style, &result));
            Ok(if t(&result, "allowed") { 0 } else { 1 })
        }
        "controls" | "pause" | "limit" | "override" | "clear" => controls_command(action, who, cmd),
        "connectors" => {
            let response = cmd.core.get("/v1/policies/connectors")?;
            if cmd.json {
                return Ok(cmd.print(&response));
            }
            cmd.lines(a(&response, "connectors").iter().map(availability_line));
            Ok(0)
        }
        "connector" => {
            let availability = args.get(2).map(String::as_str);
            let (Some(name), Some(availability)) = (who, availability.filter(|value| AVAILABILITIES.contains(value)))
            else {
                return usage(
                    "Usage: nexus policy connector <name> <disabled|available|enabled_for_all> [--teams a,b]",
                    Some("List the connectors with `nexus policy connectors`."),
                );
            };
            let body = obj(vec![
                ("availability", Some(json!(availability))),
                ("teams", cmd.flag("teams").map(|teams| json!(split_list(teams)))),
            ]);
            let saved = owner_or_admin(cmd.core.put(&format!("/v1/policies/connectors/{}", enc(name)), body))?;
            if cmd.json {
                return Ok(cmd.print(&saved));
            }
            cmd.out(format!("{} Saved {name}'s availability.", style.ok));
            cmd.out(format!("  {}", availability_line(&saved)));
            Ok(0)
        }
        "grants" => grants_command(args.get(1..).unwrap_or(&[]), cmd),
        "review-events" => review_events_command(args.get(1..).unwrap_or(&[]), cmd),
        "agent-writes" => agent_writes_command(args.get(1..).unwrap_or(&[]), cmd),
        _ => usage(
            format!("Usage: nexus policy {} …", POLICY_VERBS.join("|")),
            Some("Run `nexus policy help`."),
        ),
    }
}

/// The GitHub writes each Nexus Review mode needs, for `--for-mode`. The module's status contract
/// (`options.mode[].needs`, `nexus robustness status review`) is the source; this is its copy for a command that names
/// no repository (`REVIEW_MODE_NEEDS` in models.ts).
pub const REVIEW_MODE_NEEDS: &[(&str, &[&str])] = &[
    ("shadow", &[]),
    ("comment", &["pull_review"]),
    ("comment_and_check", &["pull_review", "check_run"]),
];

const GRANTS_SET_USAGE: &str = "Usage: nexus policy grants set <product/capability> --connector c --actions a,b | --for-mode <shadow|comment|comment_and_check>";

/// `nexus policy grants [list]` and `nexus policy grants set <product/capability> …` (models.ts `grantsCommand`).
fn grants_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let sub = args.first().map(String::as_str).unwrap_or("list");
    if sub == "list" {
        let answer = cmd.core.get("/v1/policies/grants")?;
        if cmd.json {
            return Ok(cmd.print(&answer));
        }
        let grants = a(&answer, "grants");
        if grants.is_empty() {
            cmd.out(format!("{} No module holds a write grant.", style.ok));
        }
        for grant in grants {
            cmd.out(format!(
                "{:<24}{:<10}{:<18}{}  {}",
                js_string(grant.get("module")),
                js_string(grant.get("connector")),
                js_string(grant.get("action")),
                s(grant, "grantedBy").unwrap_or(""),
                js_string(grant.get("at")),
            ));
        }
        let grantable: Vec<String> = answer
            .get("grantable")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(connector, actions)| {
                let names: Vec<&str> = actions
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                format!("{connector}: {}", names.join(", "))
            })
            .collect();
        if !grantable.is_empty() {
            cmd.out("");
            cmd.out(style.dim(&format!("Grantable  {}", grantable.join("  "))));
        }
        return Ok(0);
    }
    if sub != "set" {
        return usage(
            "Usage: nexus policy grants [list] | grants set <product/capability> …",
            Some("Run `nexus policy help`."),
        );
    }
    let target = args.get(1).map(String::as_str).unwrap_or("");
    let parts: Vec<&str> = target.split('/').collect();
    let [product, capability] = parts.as_slice() else {
        return usage(
            GRANTS_SET_USAGE,
            Some("Name the module as product/capability, like robustness/review."),
        );
    };
    if product.is_empty() || capability.is_empty() {
        return usage(
            GRANTS_SET_USAGE,
            Some("Name the module as product/capability, like robustness/review."),
        );
    }
    let mode = cmd.flag("for-mode");
    // `--actions` with no value (a switch) or `--actions ""`: revoke every write.
    let actions_given = cmd.flag("actions").is_some() || cmd.has("actions");
    if mode.is_some() && actions_given {
        return usage("--for-mode and --actions are exclusive", Some(GRANTS_SET_USAGE));
    }
    let actions: Vec<Value> = if let Some(mode) = mode {
        if target != "robustness/review" {
            return usage("--for-mode applies to robustness/review only", Some(GRANTS_SET_USAGE));
        }
        let Some((_, needs)) = REVIEW_MODE_NEEDS.iter().find(|(name, _)| *name == mode) else {
            let names: Vec<&str> = REVIEW_MODE_NEEDS.iter().map(|(name, _)| *name).collect();
            return usage(format!("--for-mode must be {}, not {mode}", names.join(", ")), None);
        };
        needs.iter().map(|need| json!(need)).collect()
    } else if actions_given {
        split_list(cmd.flag("actions").unwrap_or(""))
    } else {
        return usage(
            GRANTS_SET_USAGE,
            Some("Pass --actions (an empty one revokes every write) or --for-mode."),
        );
    };
    let connector = cmd
        .flag("connector")
        .map(str::to_string)
        .or_else(|| mode.map(|_| "github".to_string()));
    let Some(connector) = connector else {
        return usage("--connector is required with --actions", Some(GRANTS_SET_USAGE));
    };
    let body = json!({ "connector": connector, "actions": actions });
    let saved = owner_or_admin(cmd.core.put(
        &format!("/v1/policies/grants/{}/{}", enc(product), enc(capability)),
        body,
    ))?;
    if cmd.json {
        return Ok(cmd.print(&saved));
    }
    let held: Vec<&str> = a(&saved, "actions").iter().filter_map(Value::as_str).collect();
    cmd.out(format!(
        "{} {} now holds {}: {}.",
        style.ok,
        js_string(saved.get("module")),
        js_string(saved.get("connector")),
        if held.is_empty() {
            "no writes".to_string()
        } else {
            held.join(", ")
        }
    ));
    for (key, name) in [("granted", "granted"), ("revoked", "revoked")] {
        let listed: Vec<&str> = a(&saved, key).iter().filter_map(Value::as_str).collect();
        if !listed.is_empty() {
            cmd.out(format!("  {}  {}", style.dim(name), listed.join(", ")));
        }
    }
    Ok(0)
}

/// The review event levels, and what each lets a module post (`REVIEW_EVENT_TEXT` in models.ts).
const REVIEW_EVENT_LEVELS: &[(&str, &str)] = &[
    (
        "comment",
        "COMMENT reviews only: modules never approve or request changes",
    ),
    (
        "request_changes",
        "COMMENT and REQUEST_CHANGES: modules may block, never approve",
    ),
    ("full", "COMMENT, REQUEST_CHANGES and APPROVE"),
];

const REVIEW_EVENTS_USAGE: &str =
    "Usage: nexus policy review-events [show] | review-events set <comment|request_changes|full>";

fn review_event_text(level: &str) -> &'static str {
    REVIEW_EVENT_LEVELS
        .iter()
        .find(|(name, _)| *name == level)
        .map_or("", |(_, text)| text)
}

/// `nexus policy review-events [show]` and `nexus policy review-events set <level>` (models.ts `reviewEventsCommand`).
fn review_events_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let sub = args.first().map(String::as_str).unwrap_or("show");
    if sub == "show" {
        let answer = cmd.core.get("/v1/policies/review-events")?;
        if cmd.json {
            return Ok(cmd.print(&answer));
        }
        let level = s(&answer, "level").unwrap_or("");
        cmd.out(format!("{}  {}", style.bold(level), review_event_text(level)));
        let default = answer.get("default").and_then(Value::as_bool).unwrap_or(false);
        cmd.out(style.dim(&if default {
            "The default: nobody has chosen.".to_string()
        } else {
            format!(
                "Set by {} at {}.",
                s(&answer, "changedBy").unwrap_or("?"),
                s(&answer, "changedAt").unwrap_or("?")
            )
        }));
        return Ok(0);
    }
    if sub != "set" {
        return usage(REVIEW_EVENTS_USAGE, Some("Run `nexus policy help`."));
    }
    let level = args.get(1).map(String::as_str).unwrap_or("");
    if !REVIEW_EVENT_LEVELS.iter().any(|(name, _)| *name == level) {
        return usage(
            REVIEW_EVENTS_USAGE,
            Some("The level is comment, request_changes, full."),
        );
    }
    let saved = owner_or_admin(cmd.core.put("/v1/policies/review-events", json!({ "level": level })))?;
    if cmd.json {
        return Ok(cmd.print(&saved));
    }
    let level = s(&saved, "level").unwrap_or(level);
    cmd.out(format!(
        "{} Modules may now post {}.",
        style.ok,
        review_event_text(level)
    ));
    Ok(0)
}

const AGENT_WRITES_USAGE: &str = "Usage: nexus policy agent-writes [show] | agent-writes <on|off>";

/// `nexus policy agent-writes [show]` and `nexus policy agent-writes <on|off>` (models.ts `agentWritesCommand`):
/// whether the person's agent links make the organization's allowed connector writes with no record per call.
fn agent_writes_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let sub = args.first().map(String::as_str).unwrap_or("show");
    let enabled = match sub {
        "show" => None,
        "on" => Some(true),
        "off" => Some(false),
        _ => return usage(AGENT_WRITES_USAGE, Some("Run `nexus policy help`.")),
    };
    let answer = match enabled {
        None => cmd.core.get("/v1/policies/agent-writes")?,
        Some(enabled) => cmd
            .core
            .put("/v1/policies/agent-writes", json!({ "enabled": enabled }))?,
    };
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    let on = answer.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let text = if on {
        "Your agents make the connector writes your organization allows. Nexus does not ask you each time. An agent asks first, unless it runs in an auto mode. A github write runs as your own GitHub account: connect it with `nexus connect github`."
    } else {
        "Your agents make no connector write without your confirmation in Nexus."
    };
    let mark = if enabled.is_some() {
        style.ok.to_string()
    } else {
        style.bold(if on { "on" } else { "off" })
    };
    cmd.out(format!("{mark} {text}"));
    Ok(0)
}

const AVAILABILITIES: &[&str] = &["disabled", "available", "enabled_for_all"];

/// One connector's availability in the organization.
fn availability_line(entry: &Value) -> String {
    let teams = match entry.get("teams").and_then(Value::as_array) {
        Some(teams) => format!(
            "  teams {}",
            teams.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")
        ),
        None => String::new(),
    };
    let default = if t(entry, "default") { "  (default)" } else { "" };
    format!(
        "{}{}{teams}{default}",
        pad_end(s(entry, "name").unwrap_or_default(), 16),
        s(entry, "availability").unwrap_or_default()
    )
}

/// The control routes: a team's with `--team`, where its lead may act, the organization's otherwise.
fn controls_path(team: Option<&str>) -> String {
    match team {
        Some(team) => format!("/v1/policies/teams/{}/controls", enc(team)),
        None => "/v1/policies/controls".into(),
    }
}

fn controls_command(action: &str, who: Option<&str>, cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    match action {
        "controls" => {
            let response = cmd.core.get(&controls_path(cmd.args.flag_set("team")))?;
            if cmd.json {
                return Ok(cmd.print(&response));
            }
            let controls = a(&response, "controls");
            if controls.is_empty() {
                cmd.out(format!("{} No operator control is in force.", style.ok));
                return Ok(0);
            }
            cmd.lines(controls.iter().map(control_line));
            Ok(0)
        }
        "pause" | "limit" | "override" => {
            let target = control_target(who)?;
            let expires_at = control_expiry(cmd.flag("expires"), now_ms())?;
            let body = obj(vec![
                ("kind", Some(json!(action))),
                ("level", Some(json!(target.level))),
                ("subject", target.subject.map(|subject| json!(subject))),
                ("models", cmd.flag("models").map(|models| json!(split_list(models)))),
                ("reason", cmd.flag("reason").map(|reason| json!(reason))),
                ("expiresAt", Some(json!(expires_at))),
            ]);
            let control = owner_or_admin(cmd.core.post(&controls_path(cmd.args.flag_set("team")), body))?;
            if cmd.json {
                return Ok(cmd.print(&control));
            }
            cmd.out(format!(
                "{} Put a {action} on {} until {expires_at}.",
                style.ok, target.label
            ));
            cmd.out(format!("  {}", control_line(&control)));
            Ok(0)
        }
        "clear" => {
            let Some(id) = who.filter(|id| !id.is_empty()) else {
                return usage(
                    "Usage: nexus policy clear <control-id> [--team <slug>]",
                    Some("List the ids with `nexus policy controls`."),
                );
            };
            let path = format!("{}/{}", controls_path(cmd.args.flag_set("team")), enc(id));
            let control = owner_or_admin(cmd.core.delete(&path))?;
            if cmd.json {
                return Ok(cmd.print(&control));
            }
            cmd.out(format!("{} Cleared the control {id}.", style.ok));
            Ok(0)
        }
        _ => unreachable!("policy_command dispatches only the control verbs here"),
    }
}

pub fn policy_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus policy <command>", style.bold("Usage")),
        String::new(),
    ];
    lines.extend(
        policy_commands()
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 46)))),
    );
    lines.push(String::new());
    lines
        .push(style.dim(
            "Patterns: anthropic/claude-opus-5.5 (a model), anthropic/* (a provider), openai/gpt-6-* (a family).",
        ));
    lines.push(style.dim("A deny always wins; a team only narrows what its organization and parent teams allow."));
    lines.push(style.dim(
        "An allowed github write lets a member's agent do only what the member may do on GitHub: it runs as their own account.",
    ));
    lines
}

/// `nexus models [--all] [--team T]`, and `nexus models pick [<model>]`.
pub fn models_command(args: &[String], cmd: &Cmd, chat_model: Option<&str>) -> Result<i32> {
    if args.first().map(String::as_str) == Some("pick") {
        return pick_model(args.get(1).map(String::as_str), cmd, chat_model);
    }
    let style = cmd.style;
    let all = cmd.has("all");
    let team = cmd.args.flag_set("team");
    let query = search(&[
        ("status", all.then(|| "all".to_string())),
        ("team", team.map(String::from)),
    ]);
    let response = cmd.core.get(&format!("/v1/models{query}"))?;
    if cmd.json {
        return Ok(cmd.print(&response));
    }
    let data = a(&response, "data");
    if data.is_empty() {
        cmd.out(format!(
            "{} Your organization's policies allow no model{}.",
            style.warn,
            team.map(|team| format!(" in team {team}")).unwrap_or_default()
        ));
        cmd.out(format!("  {} {}", style.arrow, style.cyan("nexus policy show")));
        return Ok(0);
    }
    // Grouped by provider, in the catalog's order, each under its mark (as Nexus UI's model logos).
    let mut groups: Vec<(String, Group)> = Vec::new();
    for model in data {
        let id = js_string(model.get("id"));
        let provider = provider_of(&id);
        let at = match groups.iter().position(|(key, _)| *key == provider) {
            Some(at) => at,
            None => {
                groups.push((
                    provider.clone(),
                    Group {
                        title: Some(format!(
                            "{} {}",
                            provider_mark(style, &provider),
                            style.bold(&provider_name(&provider))
                        )),
                        rows: Vec::new(),
                    },
                ));
                groups.len() - 1
            }
        };
        let supported = s(model, "status") == Some("supported");
        let pricing = model.get("pricing").filter(|p| crate::json::truthy(p));
        groups[at].1.rows.push(Row {
            cells: vec![
                id.clone(),
                js_string(model.get("name")),
                pricing.map_or_else(|| style.dim("–"), |p| usd(n(p, "input"))),
                pricing.map_or_else(|| style.dim("–"), |p| usd(n(p, "output"))),
                if !supported {
                    style.yellow(style.g.star)
                } else if t(model, "allowed") {
                    style.ok.clone()
                } else {
                    style.fail.clone()
                },
            ],
            muted: !supported,
            selected: Some(id.as_str()) == chat_model,
            ..Row::default()
        });
    }
    let groups: Vec<Group> = groups.into_iter().map(|(_, group)| group).collect();
    cmd.lines(grouped_list(
        style,
        &[
            col("Model"),
            flex("Name"),
            right("Input / 1M"),
            right("Output / 1M"),
            col("Allowed"),
        ],
        &groups,
        None,
    ));
    let mut marks: Vec<(String, &str)> = vec![(style.ok.clone(), "allowed for you")];
    if data
        .iter()
        .any(|model| s(model, "status") == Some("supported") && !t(model, "allowed"))
    {
        marks.push((style.fail.clone(), "not allowed by your policies"));
    }
    if data.iter().any(|model| s(model, "status") != Some("supported")) {
        marks.push((style.yellow(style.g.star), "coming soon"));
    }
    if data
        .iter()
        .any(|model| s(model, "id").is_some() && s(model, "id") == chat_model)
    {
        marks.push((style.select.clone(), "your default for nexus chat"));
    }
    cmd.out("");
    cmd.out(legend(style, &marks));
    cmd.out(style.dim(if all {
        "List prices, USD per million tokens. Coming soon: listed, not callable yet."
    } else {
        "The models you may use. --all lists the whole catalog, coming soon included."
    }));
    if chat_model.is_none() {
        cmd.out(style.dim("Choose the model nexus chat uses: nexus models pick."));
    }
    Ok(0)
}

const AUTO: [&str; 4] = ["auto", "default", "nexus/auto", "nexus/default"];

fn pick_usage() -> Error {
    CliError::usage(
        "Usage: nexus models pick <model>",
        Some("Name a model or auto (`nexus models` lists them), or run it in a terminal to choose from a list."),
    )
    .into()
}

/// `nexus models pick <model>` sets the model `nexus chat` uses when no --model is given (`auto`: back to
/// nexus/auto); without a model, on a terminal, a picker chooses it.
fn pick_model(wanted: Option<&str>, cmd: &Cmd, chat_model: Option<&str>) -> Result<i32> {
    let style = cmd.style;
    let wanted = wanted.filter(|wanted| !wanted.is_empty());
    if wanted.is_none() && (cmd.json || !cmd.ctx.picker) {
        return Err(pick_usage());
    }
    let team = cmd.args.flag_set("team");
    let query = search(&[("status", Some("all".to_string())), ("team", team.map(String::from))]);
    let response = cmd.core.get(&format!("/v1/models{query}"))?;
    let data = a(&response, "data");
    let chosen: Option<&Value> = match wanted {
        Some(wanted) => {
            if AUTO.contains(&wanted.to_lowercase().as_str()) {
                None
            } else {
                Some(find_model(data, wanted, team)?)
            }
        }
        None => {
            let usable: Vec<&Value> = data
                .iter()
                .filter(|model| s(model, "status") == Some("supported") && t(model, "allowed"))
                .collect();
            let mut providers: Vec<String> = Vec::new();
            for model in &usable {
                let provider = provider_of(&js_string(model.get("id")));
                if !providers.contains(&provider) {
                    providers.push(provider);
                }
            }
            let output = |model: &Value| {
                model
                    .get("pricing")
                    .filter(|p| crate::json::truthy(p))
                    .map(|p| n(p, "output"))
            };
            let top = usable.iter().filter_map(|model| output(model)).fold(0.0, f64::max);
            let mut items = vec![PickItem {
                id: "nexus/auto".into(),
                label: "Auto".into(),
                detail: Some("nexus/auto".into()),
                group: Some("Nexus".into()),
                provider: Some("nexus".into()),
                search: Some("default".into()),
                current: chat_model.is_none(),
                facts: vec![kv("Model", "chosen by Nexus Gateway within your policies")],
                ..PickItem::default()
            }];
            for provider in &providers {
                for model in usable
                    .iter()
                    .filter(|model| provider_of(&js_string(model.get("id"))) == *provider)
                {
                    let id = js_string(model.get("id"));
                    let pricing = model.get("pricing").filter(|p| crate::json::truthy(p));
                    items.push(PickItem {
                        current: Some(id.as_str()) == chat_model,
                        label: js_string(model.get("name")),
                        detail: Some(id.clone()),
                        group: Some(provider_name(provider)),
                        provider: Some(provider.clone()),
                        meter: output(model)
                            .filter(|_| top != 0.0)
                            .map(|price| meter(style, price / top, 5, None)),
                        facts: match pricing {
                            Some(p) => vec![
                                kv("Input", format!("{} / 1M tokens", usd(n(p, "input")))),
                                kv("Output", format!("{} / 1M tokens", usd(n(p, "output")))),
                            ],
                            None => vec![kv("Price", "not listed")],
                        },
                        id,
                        ..PickItem::default()
                    });
                }
            }
            let spec = PickerSpec {
                title: "Default model for nexus chat".into(),
                meta: vec![
                    cmd.config.extra.get("org").and_then(Value::as_str).map(String::from),
                    Some(format!(
                        "{} model{} you may use",
                        usable.len(),
                        if usable.len() == 1 { "" } else { "s" }
                    )),
                ],
                items,
                action: "set default".into(),
                legend: vec![
                    (style.ok.clone(), "current default"),
                    (style.g.meter_on.to_string(), "relative price"),
                ],
            };
            let picked = run_picker(style, &spec).map_err(|error| {
                Error::Cli(CliError::new(format!(
                    "The picker could not use this terminal: {error}"
                )))
            })?;
            let Some(picked) = picked else {
                cmd.out(style.dim(&format!(
                    "No change: nexus chat still uses {}.",
                    chat_model.unwrap_or("nexus/auto")
                )));
                return Ok(0);
            };
            data.iter().find(|model| s(model, "id") == Some(picked.as_str()))
        }
    };
    let chosen_id = chosen.map(|model| js_string(model.get("id")));
    set_chat_model(cmd, chosen_id.as_deref())?;
    if cmd.json {
        return Ok(cmd.print(&json!({ "chatModel": chosen_id })));
    }
    cmd.out(match chosen {
        Some(model) => format!(
            "{} nexus chat now uses {} {} unless you name another with --model.",
            style.ok,
            style.bold(&js_string(model.get("name"))),
            style.dim(&format!("({})", js_string(model.get("id"))))
        ),
        None => format!(
            "{} nexus chat now uses {}: Nexus Gateway chooses the model within your policies.",
            style.ok,
            style.bold("nexus/auto")
        ),
    });
    Ok(0)
}

/// Save (or, with `None`, remove) the config's `chatModel`, last among its fields, as the TypeScript CLI's spread.
fn set_chat_model(cmd: &Cmd, model: Option<&str>) -> Result<()> {
    let path = &cmd.ctx.config_path;
    let mut current = load_config(path, &cmd.ctx.env_fn());
    current.extra.shift_remove("chatModel");
    if let Some(model) = model {
        current.extra.insert("chatModel".into(), Value::String(model.into()));
    }
    save_config(path, &current)
        .map_err(|error| Error::Cli(CliError::new(format!("Could not write {}: {error}", path.display()))))
}

/// A model by its id (`anthropic/claude-haiku-5-5`) or, when only one matches, by its provider's own id.
fn find_model<'a>(models: &'a [Value], wanted: &str, team: Option<&str>) -> Result<&'a Value> {
    let lower = wanted.to_lowercase();
    let hint = "`nexus models` lists the models you may use.";
    let mut found = models
        .iter()
        .find(|model| js_string(model.get("id")).to_lowercase() == lower);
    if found.is_none() {
        let suffix = format!("/{lower}");
        let matches: Vec<&Value> = models
            .iter()
            .filter(|model| {
                s(model, "provider_model").map(str::to_lowercase).as_deref() == Some(lower.as_str())
                    || js_string(model.get("id")).to_lowercase().ends_with(&suffix)
            })
            .collect();
        if matches.len() > 1 {
            return Err(CliError::usage(
                format!(
                    "{wanted} names more than one model: {}.",
                    matches
                        .iter()
                        .map(|model| js_string(model.get("id")))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Some("Name it as provider/model."),
            )
            .into());
        }
        found = matches.first().copied();
    }
    let Some(found) = found else {
        return Err(CliError::usage(format!("No model {} in the catalog.", quote(wanted)), Some(hint)).into());
    };
    let id = js_string(found.get("id"));
    if s(found, "status") != Some("supported") {
        return Err(CliError::hint(format!("{id} is coming soon, not callable yet."), hint).into());
    }
    if !t(found, "allowed") {
        return Err(CliError::hint(
            format!(
                "Your organization's policies do not allow {id}{}.",
                team.map(|team| format!(" in team {team}")).unwrap_or_default()
            ),
            format!("{hint} `nexus policy test --model {id}` says which rule refuses it."),
        )
        .into());
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_policy_targets() {
        assert_eq!(policy_target(None).expect("org").level, "organization");
        assert_eq!(
            policy_target(Some("team/web")).expect("team").subject.as_deref(),
            Some("web")
        );
        assert!(policy_target(Some("agent/cursor")).is_err());
        assert!(policy_target(Some("web")).is_err());
    }

    #[test]
    fn names_control_targets() {
        assert_eq!(control_target(Some("org")).expect("org").level, "organization");
        assert_eq!(
            control_target(Some("person/usr_1")).expect("person"),
            Target {
                level: "person",
                subject: Some("usr_1".to_string()),
                label: "person usr_1".to_string()
            }
        );
        assert_eq!(control_target(Some("key/key_a")).expect("key").level, "key");
        assert_eq!(
            control_target(Some("team/web")).expect("team").subject.as_deref(),
            Some("web")
        );
        assert!(control_target(None).is_err());
        assert!(control_target(Some("agent/claude")).is_err());
        assert!(control_target(Some("person/")).is_err());
        assert!(control_target(Some("people/usr_1")).is_err());
    }

    #[test]
    fn turns_expires_into_an_iso_time() {
        let now = 1_792_229_400_000.0;
        let hour = 3_600_000.0;
        assert_eq!(control_expiry(Some("12h"), now).expect("hours"), iso(now + 12.0 * hour));
        assert_eq!(control_expiry(Some("2d"), now).expect("days"), iso(now + 48.0 * hour));
        assert_eq!(
            control_expiry(Some("3"), now).expect("bare days"),
            iso(now + 72.0 * hour)
        );
        for bad in [
            None,
            Some(""),
            Some("0h"),
            Some("h"),
            Some("-1d"),
            Some("1.5d"),
            Some("12m"),
            Some("99999999999999999999d"),
        ] {
            assert!(control_expiry(bad, now).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn builds_a_rule_from_flags() {
        let current = json!({ "allow": ["anthropic/*"], "deny": ["anthropic/claude-fable-*"], "defaults": { "chat": "anthropic/claude-opus-5.5" } });
        let flags = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>()
        };
        let none = |_: &str| -> std::io::Result<String> { Err(std::io::Error::other("no")) };
        assert_eq!(
            rule_from_flags(
                &current,
                &flags(&[("deny", "openai/*, anthropic/claude-opus-4.6")]),
                false,
                &none
            )
            .expect("rule"),
            json!({ "allow": ["anthropic/*"], "deny": ["openai/*", "anthropic/claude-opus-4.6"], "defaults": { "chat": "anthropic/claude-opus-5.5" } })
        );
        assert_eq!(
            rule_from_flags(&current, &flags(&[("allow", "all"), ("deny", "none")]), false, &none).expect("rule"),
            json!({ "defaults": { "chat": "anthropic/claude-opus-5.5" } })
        );
        assert_eq!(
            rule_from_flags(&current, &flags(&[("default-chat", "")]), false, &none).expect("rule"),
            json!({ "allow": ["anthropic/*"], "deny": ["anthropic/claude-fable-*"] })
        );
        assert_eq!(
            rule_from_flags(&current, &flags(&[]), true, &none).expect("rule"),
            json!({})
        );
        let file = |_: &str| -> std::io::Result<String> { Ok("deny:\n  - '*'\n".into()) };
        assert_eq!(
            rule_from_flags(&current, &flags(&[("from", "r.yaml")]), false, &file).expect("rule"),
            json!({ "deny": ["*"] })
        );
    }
}
