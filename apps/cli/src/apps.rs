//! `nexus apps`, as `apps.ts` does it: Nexus Apps. A folder deployed to Vercel on Nexus's own team, one
//! Vercel project per app, read, deleted, and claimed to your own Vercel account. Off unless the Gateway sets
//! `NEXUS_APPS_ENABLED=true`.

use std::path::Path;

use base64::Engine as _;
use serde_json::{Value, json};

use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{CliError, Error, Result, usage};
use crate::json::{js_string, s};
use crate::service_accounts::asked;
use crate::style::{col, flex, kv, list, pairs, right, rows_of};

/// Folders a deploy never sends: version control, dependencies Vercel installs, and build output it makes.
pub const SKIPPED_DIRS: [&str; 5] = [".git", "node_modules", ".vercel", ".next", ".nexus"];

/// The most files and bytes Gateway takes in one deploy.
pub const MAX_FILES: usize = 5_000;
pub const MAX_BYTES: usize = 20_000_000;

pub const APPS_OFF: &str = "Nexus Apps is not turned on for this Gateway.";

const DEPLOY_USAGE: &str = "Usage: nexus apps deploy [dir] [--name N] [--framework F]";

pub fn apps_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let action = args.first().map(String::as_str).unwrap_or("");
    let id = args.get(1).map(String::as_str).filter(|id| !id.is_empty());
    match action {
        "deploy" => deploy(args.get(1).map(String::as_str), cmd),
        "cap" => {
            let (Some(id), Some(usd)) = (id, args.get(2)) else {
                return usage("Usage: nexus apps cap <id> <usd>", None);
            };
            let app = call(
                cmd.core
                    .put(&format!("/v1/apps/{}/cap", enc(id)), json!({ "monthlyCapUsd": usd })),
            )?;
            if cmd.json {
                return Ok(cmd.print(&app));
            }
            cmd.out(format!("Monthly cap: {}", dollars(s(&app, "monthlyCapUsd"))));
            Ok(0)
        }
        "list" => list_apps(cmd),
        "get" | "delete" | "claim" => {
            let Some(id) = id else {
                return usage(
                    format!("Usage: nexus apps {action} <id>"),
                    Some("`nexus apps list` prints your apps' ids."),
                );
            };
            match action {
                "get" => get(id, cmd),
                "delete" => delete(id, cmd),
                _ => claim(id, cmd),
            }
        }
        "" => usage(
            "Usage: nexus apps <deploy|list|get|cap|delete|claim>",
            Some("Run `nexus apps --help`."),
        ),
        other => usage(
            format!("Unknown command `nexus apps {other}`."),
            Some("Run `nexus apps --help`."),
        ),
    }
}

/// A call, with Nexus Apps' own refusals in words: off, not configured, the organization's cap, a deploy too large.
fn call(result: Result<Value>) -> Result<Value> {
    let result = match result {
        Err(Error::Core(error)) => match error.code.as_deref() {
            Some("apps_off") => {
                Err(CliError::hint(APPS_OFF, "An operator turns it on with NEXUS_APPS_ENABLED=true.").into())
            }
            Some("apps_unavailable") => Err(CliError::hint(
                "Nexus Apps is on, but this Gateway has no Vercel token yet.",
                "Ask the Gateway's operator.",
            )
            .into()),
            Some("apps_limit" | "app_too_large" | "vercel_refused") => Err(CliError::new(error.message.clone()).into()),
            _ => Err(Error::Core(error)),
        },
        other => other,
    };
    asked(result)
}

/// An app name from a folder's: lowercase words joined by single dashes, at most 48 characters.
pub fn app_name(folder: &str) -> String {
    let mut name = String::new();
    for c in folder.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            name.push(c);
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    let name: String = name.trim_end_matches('-').chars().take(48).collect();
    name.trim_end_matches('-').to_string()
}

/// A `.env` file holds secrets: a deploy never sends one, `.env.example` aside.
fn secret_file(name: &str) -> bool {
    (name == ".env" || name.starts_with(".env.")) && name != ".env.example"
}

/// A folder's files for a deploy, `{ path, data }` with `/` paths and base64 bytes, and the secret files left out.
pub fn collect(root: &Path, shown: &str) -> Result<(Vec<Value>, Vec<String>)> {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut bytes = 0usize;
    walk(root, "", shown, &mut files, &mut skipped, &mut bytes)?;
    Ok((files, skipped))
}

fn walk(
    dir: &Path,
    prefix: &str,
    shown: &str,
    files: &mut Vec<Value>,
    skipped: &mut Vec<String>,
    bytes: &mut usize,
) -> Result<()> {
    let unreadable = |error: std::io::Error| CliError::new(format!("Could not read {}: {error}", dir.display()));
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(unreadable)?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(unreadable)?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.file_type().map_err(unreadable)?;
        if kind.is_dir() {
            if !SKIPPED_DIRS.contains(&name.as_str()) {
                walk(&entry.path(), &path, shown, files, skipped, bytes)?;
            }
        } else if kind.is_file() {
            if name == ".DS_Store" {
                continue;
            }
            if secret_file(&name) {
                skipped.push(path);
                continue;
            }
            let data = std::fs::read(entry.path()).map_err(unreadable)?;
            *bytes += data.len();
            if *bytes > MAX_BYTES {
                return Err(CliError::usage(
                    format!("{shown} is over {} MB.", MAX_BYTES / 1_000_000),
                    Some("Deploy the built site only, or add a build step Vercel runs."),
                )
                .into());
            }
            files.push(json!({ "path": path, "data": base64::engine::general_purpose::STANDARD.encode(&data) }));
            if files.len() > MAX_FILES {
                return Err(CliError::usage(format!("{shown} has over {MAX_FILES} files."), None).into());
            }
        }
    }
    Ok(())
}

fn deploy(dir: Option<&str>, cmd: &Cmd) -> Result<i32> {
    let root = cmd.ctx.cwd.join(dir.unwrap_or("."));
    let shown = dir.map(str::to_string).unwrap_or_else(|| root.display().to_string());
    if !root.is_dir() {
        return usage(format!("{shown} is not a folder."), Some(DEPLOY_USAGE));
    }
    let folder = root
        .canonicalize()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let name = match cmd.args.flag_set("name") {
        Some(name) => name.to_string(),
        None => app_name(&folder),
    };
    if name.is_empty() {
        return usage(
            "This folder's name makes no app name.",
            Some("Name it: nexus apps deploy --name my-app"),
        );
    }
    let (files, skipped) = collect(&root, &shown)?;
    if files.is_empty() {
        return usage(format!("{shown} has no files to deploy."), None);
    }
    let count = files.len();
    let mut body = json!({ "name": name, "files": files });
    if let Some(framework) = cmd.args.flag_set("framework") {
        body["framework"] = json!(framework);
    }
    let app = call(cmd.core.post("/v1/apps", body))?;
    if cmd.json {
        return Ok(cmd.print(&app));
    }
    let id = js_string(app.get("id"));
    cmd.out(format!(
        "{} Deployed {} ({id}), {count} {}",
        cmd.style.ok,
        cmd.style.bold(&js_string(app.get("name"))),
        if count == 1 { "file" } else { "files" }
    ));
    if let Some(url) = s(&app, "url") {
        cmd.out(format!("  {url}"));
    }
    if !skipped.is_empty() {
        cmd.out(cmd.style.dim(&format!(
            "Not sent: {}. Set secrets in Vercel once you claim the app.",
            skipped.join(", ")
        )));
    }
    if s(&app, "state") != Some("READY") {
        cmd.out(
            cmd.style
                .dim(&format!("Vercel is building it. Follow it: nexus apps get {id}")),
        );
    }
    Ok(0)
}

/// A USD decimal string as dollars and cents, or `–` without one.
pub fn dollars(amount: Option<&str>) -> String {
    amount
        .and_then(|amount| amount.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .map(|value| format!("${value:.2}"))
        .unwrap_or_else(|| "–".to_string())
}

fn state(app: &Value, none: &str) -> String {
    if s(app, "pausedReason").is_some() {
        return "paused".into();
    }
    s(app, "state").unwrap_or(none).to_lowercase()
}

fn list_apps(cmd: &Cmd) -> Result<i32> {
    let answer = call(cmd.core.get("/v1/apps"))?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    let apps = answer
        .get("apps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if apps.is_empty() {
        cmd.out("No apps.");
        cmd.out(cmd.style.dim("Deploy one: nexus apps deploy ./my-site"));
        return Ok(0);
    }
    let rows = apps
        .iter()
        .map(|app| {
            vec![
                js_string(app.get("name")),
                js_string(app.get("id")),
                state(app, "–"),
                dollars(s(app, "cost30d")),
                dollars(s(app, "costToDate")),
                s(app, "url").unwrap_or("–").to_string(),
            ]
        })
        .collect();
    cmd.lines(list(
        cmd.style,
        &[
            col("App"),
            col("Id"),
            col("State"),
            right("30 days"),
            right("To date"),
            flex("URL"),
        ],
        rows_of(rows),
        None,
    ));
    Ok(0)
}

fn get(id: &str, cmd: &Cmd) -> Result<i32> {
    let app = call(cmd.core.get(&format!("/v1/apps/{}", enc(id))))?;
    if cmd.json {
        return Ok(cmd.print(&app));
    }
    let mut rows = vec![
        kv(
            "App",
            format!("{} ({})", js_string(app.get("name")), js_string(app.get("id"))),
        ),
        kv("URL", s(&app, "url").unwrap_or("–")),
        kv("State", state(&app, "not deployed")),
        kv("Billing", s(&app, "billingError").unwrap_or("current")),
        kv("Monthly cap", dollars(s(&app, "monthlyCapUsd"))),
        kv("Month spend", dollars(s(&app, "monthSpendUsd"))),
        kv("Cost, 30 days", dollars(s(&app, "cost30d"))),
        kv("Cost to date", dollars(s(&app, "costToDate"))),
        kv("Vercel project", js_string(app.get("projectName"))),
    ];
    for (label, field) in [
        ("Framework", "framework"),
        ("Deployed", "deployedAt"),
        ("Claim asked", "claimRequestedAt"),
        ("Cost read", "costUpdatedAt"),
    ] {
        if let Some(value) = s(&app, field) {
            rows.push(kv(label, value));
        }
    }
    cmd.lines(pairs(cmd.style, &rows));
    Ok(0)
}

fn delete(id: &str, cmd: &Cmd) -> Result<i32> {
    if !cmd.has("yes") {
        return usage(
            format!("Deleting {id} deletes its Vercel project and its URL stops working."),
            Some(&format!("nexus apps delete {id} --yes")),
        );
    }
    let deleted = call(cmd.core.delete(&format!("/v1/apps/{}", enc(id))))?;
    if cmd.json {
        return Ok(cmd.print(&deleted));
    }
    cmd.out(format!("{} Deleted {}.", cmd.style.ok, js_string(deleted.get("id"))));
    Ok(0)
}

fn claim(id: &str, cmd: &Cmd) -> Result<i32> {
    let answer = call(cmd.core.post(&format!("/v1/apps/{}/claim", enc(id)), json!({})))?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    cmd.out(format!(
        "{} Open this link to move {id} to your own Vercel account:",
        cmd.style.ok
    ));
    cmd.out(format!("  {}", js_string(answer.get("claimUrl"))));
    cmd.out(cmd.style.dim(&format!(
        "It works until {}. Once claimed, the app bills your Vercel account, not Nexus.",
        js_string(answer.get("expiresAt"))
    )));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_name_becomes_an_app_name() {
        assert_eq!(app_name("My Site"), "my-site");
        assert_eq!(app_name("__docs__v2"), "docs-v2");
        assert_eq!(app_name("Ünïcode"), "n-code");
        assert_eq!(app_name("!!!"), "");
        assert_eq!(app_name(&"a".repeat(60)).len(), 48);
    }

    #[test]
    fn a_deploy_sends_the_files_but_never_secrets_or_dependencies() {
        let root = std::env::temp_dir().join(format!("nexus-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("public")).expect("dir");
        std::fs::create_dir_all(root.join("node_modules/x")).expect("dir");
        std::fs::create_dir_all(root.join(".git")).expect("dir");
        std::fs::write(root.join("index.html"), "hi").expect("file");
        std::fs::write(root.join("public/logo.svg"), "<svg/>").expect("file");
        std::fs::write(root.join("node_modules/x/index.js"), "x").expect("file");
        std::fs::write(root.join(".git/HEAD"), "ref").expect("file");
        std::fs::write(root.join(".env"), "SECRET=1").expect("file");
        std::fs::write(root.join(".env.local"), "SECRET=1").expect("file");
        std::fs::write(root.join(".env.example"), "SECRET=").expect("file");
        let (files, skipped) = collect(&root, "site").expect("collected");
        let paths: Vec<&str> = files.iter().filter_map(|file| file["path"].as_str()).collect();
        assert_eq!(paths, [".env.example", "index.html", "public/logo.svg"]);
        assert_eq!(files[1]["data"], "aGk=");
        assert_eq!(skipped, [".env", ".env.local"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn costs_show_as_dollars_and_none_as_a_dash() {
        assert_eq!(dollars(Some("1.750000")), "$1.75");
        assert_eq!(dollars(Some("0.004")), "$0.00");
        assert_eq!(dollars(None), "–");
    }
}
