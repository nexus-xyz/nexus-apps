//! `nexus machines`, as `machines.ts` does it: your cloud machines in this organization, read and put to
//! sleep through `/v1/cloud/machines`. A machine belongs to one desktop session; creating, waking and deleting stay
//! with the session in the app. `--json` prints Gateway's answer as it is.
//!
//! The noun is `machines`, not `cloud`: `nexus cloud` is the Nexus Cloud product's slug in the CLI grammar.

use reqwest::Method;
use serde_json::Value;

use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{Result, usage};
use crate::json::{a, js_string, s, t};
use crate::style::{Row, ago, col, flex, kv, list, pairs};

pub const MACHINE_COMMANDS: &[(&str, &str)] = &[
    (
        "list",
        "Your live cloud machines, one per cloud session, newest first (also plain nexus machines)",
    ),
    ("show <id>", "One machine: its session, state, size and why it sleeps"),
    (
        "sleep <id>",
        "Put a machine to sleep: its disk is kept and its running time stops; the app wakes it when you use the session",
    ),
];

/// Why a machine sleeps or failed, in words; empty when there is nothing to say.
fn note(machine: &Value) -> String {
    if s(machine, "state") == Some("failed") {
        return s(machine, "error").unwrap_or("failed").to_string();
    }
    match s(machine, "stopReason") {
        Some("budget") => "compute refused by a budget".into(),
        Some("max_run") => "ran its longest with nobody using it".into(),
        Some("not_enabled") => "cloud machines are off for the organization".into(),
        Some("idle") => "idle".into(),
        Some("resize") => "restarting one size bigger".into(),
        Some(other) => other.into(),
        None => String::new(),
    }
}

/// The session's title, else the machine's name.
fn session(machine: &Value) -> String {
    s(machine, "title")
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| js_string(machine.get("name")))
}

pub fn machines_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    match action {
        "list" => {
            let answer = cmd.core.get("/v1/cloud/machines")?;
            if cmd.json {
                return Ok(cmd.print(&answer));
            }
            let machines = a(&answer, "machines");
            if machines.is_empty() {
                cmd.out("No cloud machines. A cloud session in Nexus for desktop starts one.");
                return Ok(0);
            }
            let rows: Vec<Row> = machines
                .iter()
                .map(|machine| {
                    let state = js_string(machine.get("state"));
                    let asleep = matches!(state.as_str(), "sleeping" | "gone");
                    Row {
                        cells: vec![
                            js_string(machine.get("id")),
                            if state == "ready" { style.green(&state) } else { state },
                            js_string(machine.get("size")),
                            ago(s(machine, "createdAt").unwrap_or_default()),
                            note(machine),
                            session(machine),
                        ],
                        muted: asleep,
                        ..Row::default()
                    }
                })
                .collect();
            cmd.lines(list(
                style,
                &[
                    col("Id"),
                    col("State"),
                    col("Size"),
                    col("Made"),
                    col("Why"),
                    flex("Session"),
                ],
                rows,
                None,
            ));
            Ok(0)
        }
        "show" | "sleep" => {
            let Some(id) = args.get(1) else {
                return usage(
                    format!("Usage: nexus machines {action} <id>"),
                    Some("`nexus machines` lists your machines and their ids."),
                );
            };
            let path = format!("/v1/cloud/machines/{}", enc(id));
            if action == "sleep" {
                let machine = cmd.core.call(Method::POST, &format!("{path}/sleep"), None)?;
                if cmd.json {
                    return Ok(cmd.print(&machine));
                }
                cmd.out(format!(
                    "{} {} is going to sleep: its disk is kept and its running time stops.",
                    style.ok,
                    js_string(machine.get("id"))
                ));
                return Ok(0);
            }
            let machine = cmd.core.get(&path)?;
            if cmd.json {
                return Ok(cmd.print(&machine));
            }
            let mut entries = vec![
                kv("Id", js_string(machine.get("id"))),
                kv("Session", session(&machine)),
                kv("State", js_string(machine.get("state"))),
                kv("Size", js_string(machine.get("size"))),
                kv("Made", ago(s(&machine, "createdAt").unwrap_or_default())),
            ];
            if let Some(since) = s(&machine, "sleepingSince") {
                entries.push(kv("Asleep", ago(since)));
            }
            let why = note(&machine);
            if !why.is_empty() {
                entries.push(kv("Why", why));
            }
            entries.push(kv("Host", if t(&machine, "hostOnline") { "online" } else { "offline" }));
            cmd.lines(pairs(style, &entries));
            Ok(0)
        }
        other => usage(
            format!("Unknown command `nexus machines {other}`."),
            Some("Use list, show or sleep."),
        ),
    }
}
