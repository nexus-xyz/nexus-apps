//! `nexus auth tokens`, as `tokens.ts` does it: your personal API keys and CLI sign-ins.

use serde_json::{Value, json};

use crate::artifacts::day;
use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{Result, usage};
use crate::json::{a, js_string, s, t};
use crate::service_accounts::{TOKEN_WARNING, asked};
use crate::style::{Row, col, flex, kv, list, pairs};

const CREATE_HINT: &str = "nexus auth tokens create --label <label>";

fn kind(token: &Value) -> &'static str {
    match s(token, "kind") {
        Some("api") => "API key",
        Some("cli") => "CLI sign-in",
        _ => "Nexus UI",
    }
}

pub fn tokens_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let action = args.first().map(String::as_str).unwrap_or("list");
    match action {
        "list" => list_tokens(cmd),
        "create" => create_token(cmd),
        "revoke" => revoke_token(args.get(1).map(String::as_str), cmd),
        other => usage(
            format!("Unknown command `nexus auth tokens {other}`."),
            Some("Run `nexus auth --help`."),
        ),
    }
}

fn list_tokens(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let all = cmd.has("all");
    let answer = asked(cmd.core.get(if all {
        "/v1/auth/tokens?all=1"
    } else {
        "/v1/auth/tokens"
    }))?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    // Short-lived Nexus UI sessions are not worth listing, as on Nexus UI's page.
    let tokens: Vec<&Value> = a(&answer, "tokens")
        .iter()
        .filter(|token| s(token, "kind") != Some("ui"))
        .collect();
    if tokens.is_empty() {
        cmd.out(format!("No API keys yet. Create one: {}", style.cyan(CREATE_HINT)));
        return Ok(0);
    }
    let mut columns = vec![col("Token"), flex("Name"), col("Kind")];
    if all {
        columns.push(col("Person"));
    }
    columns.extend([col("Prefix"), col("Created"), col("Expires")]);
    let rows = tokens
        .iter()
        .map(|token| {
            let mut cells = vec![
                js_string(token.get("id")),
                s(token, "label").unwrap_or(kind(token)).to_string(),
                format!("{}{}", kind(token), if t(token, "current") { " (in use)" } else { "" }),
            ];
            if all {
                cells.push(js_string(token.get("userId")));
            }
            cells.extend([
                format!("{}…", js_string(token.get("prefix"))),
                day(s(token, "createdAt")),
                day(s(token, "expiresAt")),
            ]);
            Row {
                cells,
                ..Row::default()
            }
        })
        .collect();
    cmd.lines(list(style, &columns, rows, None));
    cmd.out("");
    cmd.out(style.dim(&format!("Create an API key: {CREATE_HINT}")));
    Ok(0)
}

/// The key is in this answer only: the human output prints it once, with the warning.
fn create_token(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let Some(label) = cmd
        .args
        .flag_set("label")
        .map(str::trim)
        .filter(|label| !label.is_empty())
    else {
        return usage(
            format!("Usage: {CREATE_HINT}"),
            Some("Name it for where it goes, e.g. --label \"CI deploys\"."),
        );
    };
    let created = asked(cmd.core.post("/v1/auth/tokens", json!({ "label": label })))?;
    if cmd.json {
        return Ok(cmd.print(&created));
    }
    cmd.out(format!(
        "{} Created API key {}",
        style.ok,
        style.bold(s(&created, "label").unwrap_or(label))
    ));
    cmd.lines(pairs(
        style,
        &[
            kv("Id", js_string(created.get("id"))),
            kv("Token", js_string(created.get("token"))),
        ],
    ));
    cmd.out(format!("{} {TOKEN_WARNING}", style.warn));
    cmd.out(style.dim("Sign in with it in CI: echo \"$NEXUS_API_KEY\" | nexus auth login --with-token"));
    Ok(0)
}

fn revoke_token(id: Option<&str>, cmd: &Cmd) -> Result<i32> {
    let Some(id) = id.filter(|id| !id.is_empty()) else {
        return usage(
            "Usage: nexus auth tokens revoke <id>",
            Some("`nexus auth tokens` lists your tokens and their ids."),
        );
    };
    let revoked = asked(cmd.core.delete(&format!("/v1/auth/tokens/{}", enc(id))))?;
    if cmd.json {
        return Ok(cmd.print(&revoked));
    }
    cmd.out(format!(
        "{} Revoked {id}: anything using it stops working now.",
        cmd.style.ok
    ));
    Ok(0)
}
