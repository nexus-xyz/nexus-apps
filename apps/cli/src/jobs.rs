//! `nexus jobs`, as `jobs.ts` does it: a module's verb run in the background, read, cancelled, and its
//! result.

use serde_json::{Map, Value, json};

use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{Result, usage};
use crate::json::{js_string, pretty, s};
use crate::service_accounts::asked;
use crate::style::{kv, pairs};

const SUBMIT_USAGE: &str = "Usage: nexus jobs submit <product>/<capability> <verb> [--input <json>] [--team <slug>]";

pub fn jobs_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let action = args.first().map(String::as_str).unwrap_or("");
    let id = args.get(1).map(String::as_str).filter(|id| !id.is_empty());
    match action {
        "submit" => submit(args.get(1..).unwrap_or(&[]), cmd),
        "get" | "cancel" | "result" => {
            let Some(id) = id else {
                return usage(
                    format!("Usage: nexus jobs {action} <id>"),
                    Some("`nexus jobs submit` prints a job's id."),
                );
            };
            match action {
                "get" => get(id, cmd),
                "cancel" => cancel(id, cmd),
                _ => result(id, cmd),
            }
        }
        "" => usage(
            "Usage: nexus jobs <submit|get|cancel|result>",
            Some("Run `nexus jobs --help`."),
        ),
        other => usage(
            format!("Unknown command `nexus jobs {other}`."),
            Some("Run `nexus jobs --help`."),
        ),
    }
}

fn submit(args: &[String], cmd: &Cmd) -> Result<i32> {
    let (Some(module), Some(verb)) = (args.first(), args.get(1)) else {
        return usage(SUBMIT_USAGE, Some("`nexus modules` lists the modules and their verbs."));
    };
    let input = match cmd.args.flag_set("input") {
        None => Value::Object(Map::new()),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(input @ Value::Object(_)) => input,
            _ => return usage("--input needs a JSON object, e.g. '{\"repo\":\"…\"}'.", None),
        },
    };
    let mut body = json!({ "module": module, "verb": verb, "input": input });
    if let Some(team) = cmd.args.flag_set("team") {
        body["team"] = json!(team);
    }
    let job = asked(cmd.core.post("/v1/jobs", body))?;
    if cmd.json {
        return Ok(cmd.print(&job));
    }
    let id = js_string(job.get("id"));
    cmd.out(format!(
        "{} Submitted job {} ({})",
        cmd.style.ok,
        cmd.style.bold(&id),
        js_string(job.get("state"))
    ));
    cmd.out(cmd.style.dim(&format!("Follow it: nexus jobs get {id}")));
    Ok(0)
}

fn get(id: &str, cmd: &Cmd) -> Result<i32> {
    let job = asked(cmd.core.get(&format!("/v1/jobs/{}", enc(id))))?;
    if cmd.json {
        return Ok(cmd.print(&job));
    }
    let mut rows = vec![
        kv("Job", js_string(job.get("id"))),
        kv(
            "Verb",
            format!("{} {}", js_string(job.get("module")), js_string(job.get("verb"))),
        ),
        kv("State", js_string(job.get("state"))),
    ];
    for (label, field) in [
        ("Team", "teamId"),
        ("Created", "createdAt"),
        ("Started", "startedAt"),
        ("Finished", "finishedAt"),
        ("Failure", "failureKind"),
    ] {
        if let Some(value) = s(&job, field) {
            rows.push(kv(label, value));
        }
    }
    cmd.lines(pairs(cmd.style, &rows));
    Ok(0)
}

fn cancel(id: &str, cmd: &Cmd) -> Result<i32> {
    let job = asked(cmd.core.post(&format!("/v1/jobs/{}/cancel", enc(id)), json!({})))?;
    if cmd.json {
        return Ok(cmd.print(&job));
    }
    match s(&job, "state") {
        Some("cancelled") => cmd.out(format!("{} Cancelled {id}.", cmd.style.ok)),
        state => cmd.out(format!(
            "{} {id} is {}: only a job no worker has taken is cancelled.",
            cmd.style.warn,
            state.unwrap_or("unknown")
        )),
    }
    Ok(0)
}

fn result(id: &str, cmd: &Cmd) -> Result<i32> {
    let answer = asked(cmd.core.get(&format!("/v1/jobs/{}/result", enc(id))))?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    match answer.get("result") {
        Some(result) if !result.is_null() => cmd.out(pretty(result)),
        _ => cmd.out(format!(
            "{id} is {}, with no result.",
            s(&answer, "state").unwrap_or("unknown")
        )),
    }
    Ok(0)
}
