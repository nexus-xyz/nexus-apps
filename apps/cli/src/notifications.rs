//! `nexus notifications`, as `notifications.ts` does it: your inbox in this organization, read and marked
//! through the notifications API (Rust gateway only). `--json` prints Gateway's answer as it is.

use reqwest::Method;
use serde_json::Value;

use crate::cli::Cmd;
use crate::client::{enc, search};
use crate::error::{Result, usage};
use crate::json::{a, js_string, s, t};
use crate::style::{Row, ago, col, flex, list, rows_of};

pub const NOTIFICATION_COMMANDS: &[(&str, &str)] = &[
    (
        "list [--unread] [--group G] [--cursor C]",
        "Your items not yet done, newest first (also plain nexus notifications); --unread: only those not yet read",
    ),
    ("read <id>", "Mark an item read"),
    ("done <id>", "Mark an item done, and read: it leaves your inbox"),
    (
        "kinds",
        "The catalogue: each kind of notification, its group, whether it is required, and its default channels",
    ),
];

/// A group's name as Nexus UI shows it.
fn group_label(group: Option<&str>) -> String {
    match group {
        Some("my_sessions") => "My sessions".into(),
        Some("usage_and_billing") => "Usage and billing".into(),
        Some("requests_and_approvals") => "Requests and approvals".into(),
        Some("automations_and_integrations") => "Automations and integrations".into(),
        Some("account_and_security") => "Account and security".into(),
        other => other.unwrap_or_default().into(),
    }
}

pub fn notifications_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    match action {
        "list" => {
            let query = search(&[
                ("unread", cmd.has("unread").then(|| "true".to_string())),
                ("group", cmd.flag_set_str("group")),
                ("cursor", cmd.flag_set_str("cursor")),
            ]);
            let page = cmd.core.get(&format!("/v1/notifications{query}"))?;
            if cmd.json {
                return Ok(cmd.print(&page));
            }
            let items = a(&page, "items");
            if items.is_empty() {
                cmd.out(if cmd.has("unread") {
                    "Nothing unread in your inbox."
                } else {
                    "Nothing in your inbox."
                });
                return Ok(0);
            }
            let rows: Vec<Row> = items
                .iter()
                .map(|item| {
                    let read = s(item, "readAt").is_some();
                    let text = js_string(item.get("subjectText"));
                    let count = item.get("count").and_then(Value::as_i64).unwrap_or(1);
                    Row {
                        cells: vec![
                            js_string(item.get("id")),
                            group_label(s(item, "group")),
                            ago(s(item, "updatedAt").unwrap_or_default()),
                            if read {
                                "read".to_string()
                            } else {
                                style.yellow("unread")
                            },
                            if count > 1 { format!("{text} ({count})") } else { text },
                        ],
                        muted: read,
                        ..Row::default()
                    }
                })
                .collect();
            cmd.lines(list(
                style,
                &[col("Id"), col("Group"), col("When"), col("Status"), flex("What")],
                rows,
                None,
            ));
            if let Some(next) = s(&page, "nextCursor") {
                cmd.out("");
                cmd.out(style.dim(&format!("More: nexus notifications list --cursor {next}")));
            }
            Ok(0)
        }
        "read" | "done" => {
            let Some(id) = args.get(1) else {
                return usage(
                    format!("Usage: nexus notifications {action} <id>"),
                    Some("`nexus notifications` lists your items and their ids."),
                );
            };
            let item = cmd
                .core
                .call(Method::POST, &format!("/v1/notifications/{}/{action}", enc(id)), None)?;
            if cmd.json {
                return Ok(cmd.print(&item));
            }
            let left = if action == "done" { ": it left your inbox" } else { "" };
            cmd.out(format!(
                "{} Marked {} {action}{left}.",
                style.ok,
                js_string(item.get("id"))
            ));
            Ok(0)
        }
        "kinds" => {
            let response = cmd.core.get("/v1/notifications/kinds")?;
            if cmd.json {
                return Ok(cmd.print(&response));
            }
            let rows: Vec<Vec<String>> = a(&response, "kinds")
                .iter()
                .map(|kind| {
                    let channels: Vec<&str> = a(kind, "defaults")
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|channel| matches!(*channel, "inbox" | "email"))
                        .collect();
                    vec![
                        js_string(kind.get("name")),
                        group_label(s(kind, "group")),
                        if t(kind, "required") { "yes" } else { "no" }.to_string(),
                        if channels.is_empty() {
                            "—".to_string()
                        } else {
                            channels.join(", ")
                        },
                        js_string(kind.get("subjectText")),
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[
                    col("Kind"),
                    col("Group"),
                    col("Required"),
                    col("Channels"),
                    flex("Text"),
                ],
                rows_of(rows),
                None,
            ));
            Ok(0)
        }
        other => usage(
            format!("Unknown command `nexus notifications {other}`."),
            Some("Use list, read, done or kinds."),
        ),
    }
}
