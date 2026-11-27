//! `nexus keys history|replace|disable|enable|remove <provider>`: a provider key's versions, and an
//! owner's or admin's changes to it, each with `--dry-run`. `--team` or `--user` names a team's or a person's key.
//! `provider-keys.ts` in the TypeScript CLI.

use serde_json::{Value, json};

use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{Result, usage};
use crate::json::{a, js_string, s, t};
use crate::style::{Style, ago, col, flex, list, rows_of};

/// The holder `--team <slug|id>` or `--user <me|id>` names, as the routes take it; none is the
/// organization's own key. Both at once is a usage error, before any request.
pub fn holder(cmd: &Cmd) -> Result<Vec<(&'static str, String)>> {
    let holder: Vec<(&'static str, String)> = ["team", "user"]
        .into_iter()
        .filter_map(|name| cmd.flag_set_str(name).map(|value| (name, value)))
        .collect();
    if holder.len() > 1 {
        return usage(
            "--team and --user name two holders: give one.",
            Some("A team's key: --team <slug|id>. A person's: --user <me|id>."),
        );
    }
    Ok(holder)
}

/// `body` with the holder's fields.
pub fn with_holder(mut body: Value, holder: &[(&str, String)]) -> Value {
    if let Some(object) = body.as_object_mut() {
        for (name, value) in holder {
            object.insert((*name).to_string(), json!(value));
        }
    }
    body
}

/// Who holds the key, for a message: ` for team web`, ` for you`, ` for usr_…`, or nothing.
pub fn holder_words(holder: &[(&str, String)]) -> String {
    match holder.first() {
        Some((name, team)) if *name == "team" => format!(" for team {team}"),
        Some((_, user)) if user == "me" => " for you".to_string(),
        Some((_, user)) => format!(" for {user}"),
        None => String::new(),
    }
}

/// The holder's flags, for a command a message suggests: ` --team web`, or nothing.
pub fn holder_flags(holder: &[(&str, String)]) -> String {
    holder
        .iter()
        .map(|(name, value)| format!(" --{name} {value}"))
        .collect()
}

/// `nexus keys history <provider>`: each key stored for the provider, oldest first, never the value.
pub fn history(provider: &str, cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let query: String = holder(cmd)?
        .iter()
        .enumerate()
        .map(|(n, (name, value))| format!("{}{name}={}", if n == 0 { "?" } else { "&" }, enc(value)))
        .collect();
    let key = cmd.core.get(&format!("/v1/keys/providers/{}{query}", enc(provider)))?;
    if cmd.json {
        return Ok(cmd.print(&key));
    }
    let rows: Vec<Vec<String>> = a(&key, "versions")
        .iter()
        .map(|version| {
            vec![
                js_string(version.get("version")),
                format!("••••{}", js_string(version.get("last4"))),
                ago(s(version, "addedAt").unwrap_or("")),
                s(version, "addedBy").unwrap_or("–").to_string(),
            ]
        })
        .collect();
    cmd.lines(list(
        style,
        &[col("Version"), col("Key"), col("Added"), flex("By")],
        rows_of(rows),
        None,
    ));
    cmd.out(format!("{} {}", style.dim("Now:"), status(style, &key)));
    Ok(0)
}

/// `nexus keys replace <provider> <key>` (`key` set) or `disable|enable|remove <provider>`, with `--dry-run`.
pub fn change(verb: &str, provider: &str, key: Option<&str>, cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let dry_run = cmd.has("dry-run");
    let holder = holder(cmd)?;
    let body = match key {
        Some(key) => json!({ "key": key, "dryRun": dry_run }),
        None => json!({ "dryRun": dry_run }),
    };
    let answer = cmd.core.post(
        &format!("/v1/keys/providers/{}/{verb}", enc(provider)),
        with_holder(body, &holder),
    )?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    let stored = answer.get("providerKey").cloned().unwrap_or(Value::Null);
    let named = format!(
        "{provider} key {}{}{}",
        style.dim("••••"),
        js_string(stored.get("last4")),
        holder_words(&holder)
    );
    // A replaced or enabled key was checked again, so its state is news; a disabled or removed key's is not.
    let checked = match s(&stored, "state") {
        Some("unreachable") if key.is_some() || verb == "enable" => " (not checked by the provider)",
        Some("valid") if key.is_some() || verb == "enable" => " (checked)",
        // A replacement keeps a disabled key disabled.
        Some("disabled") if key.is_some() => " (still disabled)",
        _ => "",
    };
    let done = match verb {
        "replace" => "Replaced",
        "disable" => "Disabled",
        "enable" => "Enabled",
        _ => "Removed",
    };
    cmd.out(if !t(&answer, "changed") {
        let already = if verb == "enable" { "enabled" } else { "disabled" };
        format!("{} Your {named} is already {already}.", style.ok)
    } else if dry_run {
        format!(
            "{} Would {verb} your {named}{checked}. Run without --dry-run to {verb} it.",
            style.warn
        )
    } else if verb == "remove" {
        format!(
            "{} Removed your {named}. Its versions stay: {}",
            style.ok,
            style.cyan(&format!("nexus keys history {provider}{}", holder_flags(&holder)))
        )
    } else {
        format!("{} {done} your {named}{checked}.", style.ok)
    });
    Ok(0)
}

/// A key's status in `nexus keys list` and `history`.
pub fn status(style: &Style, key: &Value) -> String {
    match s(key, "state") {
        Some("disabled") => return format!("{} {}", style.warn, style.yellow("disabled: no call uses it")),
        Some("removed") => return style.dim("removed"),
        _ => {}
    }
    if t(key, "unused") {
        let notice = s(key, "notice").unwrap_or("no longer used");
        return format!("{} {}", style.warn, style.yellow(&format!("unused: {notice}")));
    }
    match s(key, "state") {
        Some("unreachable") => format!("{} {}", style.warn, style.yellow("not checked by the provider")),
        Some("rejected") => format!(
            "{} {}",
            style.warn,
            style.yellow("rejected by the provider: replace it, or enable it to check it again")
        ),
        _ => format!("{} in use", style.ok),
    }
}

/// When a call last used the key: a day, or never.
pub fn last_used(key: &Value) -> String {
    s(key, "lastUsedOn").unwrap_or("never").to_string()
}
