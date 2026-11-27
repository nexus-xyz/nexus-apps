//! `nexus <product> status <capability> --repo R [--pr N] [--json]`: what a capability will do, and why
//! (`module-status.ts`). The module computes the answer (Nexus Review's status contract); this only prints it and
//! never re-derives it. A key it does not know is skipped.

use serde_json::Value;

use crate::client::search;
use crate::json::{a, js_string, s};
use crate::style::Style;

/// The query of `GET …/status`: the inputs the manifest declares for `status` (every verb when it lists none).
pub fn status_query(manifest: &Value, input: &[(String, String)]) -> String {
    let mut query: Vec<(String, Option<String>)> = Vec::new();
    for declared in a(manifest, "inputs") {
        let name = js_string(declared.get("name"));
        let Some((_, value)) = input.iter().find(|(key, _)| *key == name) else {
            continue;
        };
        let for_status = match declared.get("verbs").and_then(Value::as_array) {
            Some(verbs) => verbs.iter().any(|verb| verb.as_str() == Some("status")),
            None => true,
        };
        if for_status {
            query.push((name.replace('-', "_"), Some(value.clone())));
        }
    }
    let borrowed: Vec<(&str, Option<String>)> =
        query.iter().map(|(key, value)| (key.as_str(), value.clone())).collect();
    search(&borrowed)
}

/// The order the behaviours print in; any other `effective` entry follows.
const EFFECTIVE_ORDER: &[&str] = &[
    "automatic",
    "manual",
    "posting",
    "comments",
    "check_run",
    "replies_reactions",
    "resolve_threads",
    "effort",
    "apply",
];

fn mark(style: &Style, state: &str) -> String {
    match state {
        "on" => style.ok.clone(),
        "blocked" => style.fail.clone(),
        "fallback" => style.warn.clone(),
        _ => style.info.clone(),
    }
}

fn label(key: &str) -> String {
    key.replace('_', " ")
}

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    s(value, key).unwrap_or("")
}

fn reason_lines(style: &Style, reasons: Option<&Value>, indent: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for reason in reasons.and_then(Value::as_array).into_iter().flatten() {
        lines.push(format!(
            "{indent}{}  {}",
            style.dim(str_of(reason, "code")),
            str_of(reason, "message")
        ));
        let fix = reason.get("fix").unwrap_or(&Value::Null);
        if let Some(command) = s(fix, "command").filter(|command| !command.is_empty()) {
            lines.push(format!("{indent}  {} {}", style.arrow, style.cyan(command)));
        } else if let Some(action) = s(fix, "action").filter(|action| !action.is_empty()) {
            lines.push(format!("{indent}  {} {action}", style.arrow));
        }
    }
    lines
}

/// The human summary of a status answer.
pub fn status_lines(style: &Style, status: &Value) -> Vec<String> {
    let null = Value::Null;
    let effective = status.get("effective").unwrap_or(&null);
    let repository = status.get("repository").unwrap_or(&null);
    let mut lines = Vec::new();
    let title = [str_of(repository, "repo"), str_of(effective, "readiness")]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("  ");
    if !title.is_empty() {
        lines.push(style.bold(&title));
    }
    if let Some(headline) = s(effective, "headline").filter(|headline| !headline.is_empty()) {
        lines.push(headline.to_string());
    }
    if let Some(entries) = effective.as_object() {
        let mut keys: Vec<&str> = EFFECTIVE_ORDER
            .iter()
            .copied()
            .filter(|key| entries.contains_key(*key))
            .collect();
        keys.extend(
            entries
                .keys()
                .map(String::as_str)
                .filter(|key| !EFFECTIVE_ORDER.contains(key)),
        );
        let behaviours: Vec<&str> = keys
            .into_iter()
            .filter(|key| entries[*key].get("state").is_some_and(Value::is_string))
            .collect();
        if !behaviours.is_empty() {
            lines.push(String::new());
            for key in behaviours {
                let entry = &entries[key];
                lines.push(format!(
                    "{} {:<18}{}",
                    mark(style, str_of(entry, "state")),
                    label(key),
                    str_of(entry, "summary")
                ));
            }
        }
    }
    if !a(status, "warnings").is_empty() {
        lines.push(String::new());
        lines.push(style.bold("Warnings"));
        lines.extend(reason_lines(style, status.get("warnings"), "  "));
    }
    let run = status.get("run").unwrap_or(&null);
    if run.as_object().is_some_and(|run| !run.is_empty()) {
        lines.push(String::new());
        lines.push(style.bold("Manual runs"));
        let post = run.get("post").unwrap_or(&null);
        if !str_of(post, "summary").is_empty() {
            lines.push(format!(
                "  {} post to GitHub  {}",
                mark(style, str_of(post, "state")),
                str_of(post, "summary")
            ));
        }
        for (key, gap) in [("skips", "    "), ("applies", "  ")] {
            let listed: Vec<String> = a(run, key).iter().filter_map(Value::as_str).map(label).collect();
            if !listed.is_empty() {
                lines.push(format!("  {}{gap}{}", style.dim(key), listed.join(", ")));
            }
        }
        let pr = run.get("pr").unwrap_or(&null);
        if let Some(number) = pr.get("number").and_then(Value::as_u64) {
            let author = s(pr, "author")
                .filter(|author| !author.is_empty())
                .unwrap_or("unknown author");
            lines.push(match pr.get("reviewed") {
                Some(Value::Bool(false)) => {
                    format!("  {} #{number} by {author}: the author filter skips it", style.warn)
                }
                Some(Value::Bool(true)) => {
                    format!("  {} #{number} by {author}: a manual run reviews it", style.ok)
                }
                _ => match s(pr, "error").filter(|error| !error.is_empty()) {
                    Some(error) => format!("  {} #{number}: unknown, it was not read: {error}", style.warn),
                    None => format!("  {} #{number}: unknown, it was not read", style.warn),
                },
            });
            lines.extend(reason_lines(style, pr.get("reasons"), "    "));
        }
        let overrides = run.get("overrides").unwrap_or(&null);
        if overrides.get("available") == Some(&Value::Bool(false)) {
            let why = overrides
                .get("reason")
                .and_then(|reason| s(reason, "message"))
                .map(|message| format!(": {message}"))
                .unwrap_or_default();
            lines.push(format!("  {}  off{why}", style.dim("overrides")));
        }
        if let Some(note) = run.get("model_override").filter(|note| note.is_object()) {
            lines.push(format!("  {}  {}", style.dim("--model"), str_of(note, "message")));
        }
    }
    if lines.is_empty()
        && let Some(entries) = status.as_object()
    {
        for (key, value) in entries {
            let text = if value.is_object() || value.is_array() {
                value.to_string()
            } else {
                js_string(Some(value))
            };
            lines.push(format!("{}  {text}", style.dim(key)));
        }
    }
    lines
}

/// What a 404 on `status` means: the module (or the Gateway) predates the status contract.
pub fn legacy_status_message(capability_name: &str) -> String {
    format!(
        "This Gateway's {capability_name} predates the status contract: it cannot say what will run. Upgrade the \
         Gateway, or read the settings with `connect`."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pr_line(pr: Value) -> String {
        let status = json!({ "run": { "pr": pr } });
        status_lines(&Style::new(false), &status).join("\n")
    }

    #[test]
    fn a_pull_request_that_was_not_read_is_unknown_not_reviewed() {
        let unread = pr_line(json!({ "number": 7, "author": "ana", "reviewed": null, "error": "github is off" }));
        assert!(
            unread.contains("#7: unknown, it was not read: github is off"),
            "{unread}"
        );
        let reviewed = pr_line(json!({ "number": 7, "author": "ana", "reviewed": true }));
        assert!(reviewed.contains("#7 by ana: a manual run reviews it"), "{reviewed}");
        let skipped = pr_line(json!({ "number": 7, "author": "ana", "reviewed": false }));
        assert!(skipped.contains("#7 by ana: the author filter skips it"), "{skipped}");
    }
}
