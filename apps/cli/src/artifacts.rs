//! `nexus artifacts` (`artifacts.ts`, Nexus Artifacts): chats shared as immutable snapshots, inside the organization
//! (the whole organization, chosen teams or chosen people) or outside it (named people by verified email, or anyone with
//! the link), as the organization's settings allow. Shared with `nexus chat share`. `nexus artifacts images` saves the
//! images of an artifact's questions to files (`images.rs`), through a public link's token too.
//! Also what `nexus chat` shares with it: how a conversation prints, an audience in words, and Markdown export.

use serde_json::{Value, json};

use crate::catalog::ARTIFACT_COMMANDS;
use crate::cli::Cmd;
use crate::client::{enc, search};
use crate::config::{Config, load_config, save_config, ui_url};
use crate::error::{Result, usage};
use crate::images::{image_detail, save_images};
use crate::json::{a, compact, js_string, n, s, t, truthy};
use crate::style::{Style, ago, col, js_len, kv, list, pad_end, pairs, right, rows_of, usd};

pub fn artifacts_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus artifacts <command>", style.bold("Usage")),
        String::new(),
        "Artifacts are chats shared as snapshots: inside your organization (everyone, chosen teams or chosen people) or"
            .to_string(),
        "outside it (named people by email, or anyone with the link), as your organization's settings allow.".to_string(),
        format!(
            "Share one with {}.",
            style.cyan("nexus chat share <session> --org | --team T | --people a@x,b@y | --to a@x,b@y | --public")
        ),
        String::new(),
        style.bold("Commands"),
    ];
    lines.extend(
        ARTIFACT_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 56)))),
    );
    lines
}

/// The command names of `nexus artifacts help --json`, once each.
pub fn artifact_command_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for (name, _) in ARTIFACT_COMMANDS {
        let first = name.split(' ').next().unwrap_or("");
        if !names.contains(&first) {
            names.push(first);
        }
    }
    names
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    a(value, key).iter().map(|item| js_string(Some(item))).collect()
}

/// Who may read it, in words: "your organization", "team web", "teams web, api", "ada@acme.test, bo@acme.test",
/// "ada@partner.test (outside)", "anyone with the link".
pub fn audience_text(audience: &Value) -> String {
    match s(audience, "kind") {
        Some("org") => "your organization".into(),
        Some("teams") => {
            let teams = strings(audience, "teams");
            format!(
                "{} {}",
                if teams.len() == 1 { "team" } else { "teams" },
                teams.join(", ")
            )
        }
        Some("public") => "anyone with the link".into(),
        Some("external") => {
            let people = strings(audience, "people");
            if people.is_empty() {
                "named people outside the organization".into()
            } else {
                format!("{} (outside)", people.join(", "))
            }
        }
        _ => strings(audience, "people").join(", "),
    }
}

/// How an artifact is shared outside, in words: "named (2 people)", "public link, expires 2026-11-17T…"; else none.
pub fn outside_text(artifact: &Value) -> Option<String> {
    let audience = artifact.get("audience").unwrap_or(&Value::Null);
    match s(audience, "kind") {
        Some("external") => {
            let count = a(audience, "people").len();
            Some(if count == 0 {
                "named people".into()
            } else {
                format!("named ({count} {})", if count == 1 { "person" } else { "people" })
            })
        }
        Some("public") => Some(match s(artifact, "expiresAt").filter(|at| !at.is_empty()) {
            Some(at) => format!("public link, expires {at}"),
            None => "public link, never expires".into(),
        }),
        _ => None,
    }
}

/// A setting's or a cell's value in words: strings as they are, `–` for none, anything else as JSON.
pub fn value_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "–".into(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => compact(other),
    }
}

/// A comma-separated list of emails from a flag.
fn emails(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(String::from)
        .collect()
}

/// A day from an ISO time (`2026-11-17`), or `–`.
pub(crate) fn day(iso: Option<&str>) -> String {
    match iso.filter(|iso| !iso.is_empty()) {
        Some(iso) => iso.chars().take(10).collect(),
        None => "–".into(),
    }
}

pub fn author_text(author: Option<&Value>, mine: bool) -> String {
    if mine {
        return "you".into();
    }
    let Some(author) = author.filter(|author| author.is_object()) else {
        return "–".into();
    };
    let name = s(author, "name").filter(|name| !name.is_empty());
    let email = s(author, "email").filter(|email| !email.is_empty());
    match (name, email) {
        (Some(name), Some(email)) if name != email => format!("{name} <{email}>"),
        (_, Some(email)) => email.to_string(),
        (Some(name), None) => name.to_string(),
        (None, None) => "–".into(),
    }
}

/// A link to a page of Nexus UI in the signed-in organization (`uiLink` in `config.ts`): `<ui>/<org><path>`, or
/// `<ui>?next=<path>` for a config saved before the organization was kept.
pub fn ui_link(config: &Config, path: &str) -> String {
    let base = config.ui_url.clone().unwrap_or_else(|| ui_url(&config.url, &|_| None));
    if let Some(org) = config
        .extra
        .get("org")
        .and_then(Value::as_str)
        .filter(|org| !org.is_empty())
    {
        return format!("{base}/{}{path}", enc(org));
    }
    let (page, hash) = match path.find('#') {
        Some(at) => (&path[..at], &path[at..]),
        None => (path, ""),
    };
    format!("{base}?next={}{hash}", enc(page))
}

/// The fast viewer's link (`viewer_url` in Gateway's `Links`): `<origin>/a/<key>`, the origin being Nexus UI's URL
/// without its `/app`. A page's id opens there for anyone who may read the page, in no one organization.
pub fn viewer_link(config: &Config, key: &str) -> String {
    let base = config.ui_url.clone().unwrap_or_else(|| ui_url(&config.url, &|_| None));
    let base = base.trim_end_matches('/');
    format!("{}/a/{}", base.strip_suffix("/app").unwrap_or(base), enc(key))
}

/// Keep the session `nexus chat` continues.
pub fn remember(cmd: &Cmd, session: Option<&str>) {
    let mut current = load_config(&cmd.ctx.config_path, &cmd.ctx.env_fn());
    if current.chat_session.as_deref() == session {
        return;
    }
    current.chat_session = session.map(String::from);
    let _ = save_config(&cmd.ctx.config_path, &current);
}

pub fn print_sources(message: &Value, cmd: &Cmd) {
    let style = cmd.style;
    let citations = a(message, "citations");
    if citations.is_empty() {
        return;
    }
    cmd.out("");
    cmd.out(style.bold("Sources"));
    for source in citations {
        let label = ["repo", "page", "section"]
            .iter()
            .filter_map(|key| {
                source
                    .get(*key)
                    .filter(|v| crate::json::truthy(v))
                    .map(|v| js_string(Some(v)))
            })
            .collect::<Vec<_>>()
            .join(" › ");
        let files = a(source, "files");
        cmd.out(format!(
            "  [{}] {label}  {}{}",
            js_string(source.get("n")),
            style.dim(&js_string(source.get("path"))),
            if files.is_empty() {
                String::new()
            } else {
                style.dim(&format!(
                    "  {}",
                    files.iter().map(|f| js_string(Some(f))).collect::<Vec<_>>().join(", ")
                ))
            }
        ));
    }
}

/// `[Image #1] shot.png (1600 × 1000, 312 KB)` (`imageLines` in artifacts.ts): a question's images, as Claude Code
/// shows a pasted one.
pub fn image_lines(attachments: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(images)) = attachments else {
        return Vec::new();
    };
    images
        .iter()
        .enumerate()
        .map(|(index, image)| {
            let name = s(image, "name").or_else(|| s(image, "mediaType")).unwrap_or("image");
            format!("[Image #{}] {name} ({})", index + 1, image_detail(image))
        })
        .collect()
}

/// A conversation as `nexus chat sessions show` prints it: each question, then each answer with its model, cost and
/// sources.
pub fn print_messages(messages: &[Value], cmd: &Cmd) {
    let style = cmd.style;
    for message in messages {
        cmd.out("");
        let content = js_string(message.get("content"));
        if s(message, "role") == Some("user") {
            cmd.out(style.cyan("You"));
            if !content.is_empty() {
                cmd.out(content);
            }
            for line in image_lines(message.get("attachments")) {
                cmd.out(style.dim(&line));
            }
            continue;
        }
        let label = match s(message, "status") {
            Some("refused") => style.red("Refused"),
            Some("failed") => style.yellow("Failed"),
            _ => style.bold("Nexus"),
        };
        let cost = n(message, "costUsd");
        cmd.out(format!(
            "{label} {}",
            style.dim(&format!(
                "{}{}",
                s(message, "model").unwrap_or(""),
                if cost != 0.0 {
                    format!(" · {}", usd(cost))
                } else {
                    String::new()
                }
            ))
        ));
        cmd.out(content);
        print_sources(message, cmd);
    }
}

/// The session's sharing state as `label value` pairs (`nexus chat sessions show`).
pub fn state_rows(session: &Value) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(author) = session.get("author").filter(|author| author.is_object()) {
        rows.push(kv("Author", author_text(Some(author), false)));
    }
    let sharing = session.get("sharing").filter(|sharing| sharing.is_object());
    if let Some(visibility) = s(session, "visibility") {
        let text = match (visibility, sharing) {
            ("shared", Some(sharing)) => format!(
                "Shared with {} (version {}{})",
                audience_text(sharing.get("audience").unwrap_or(&Value::Null)),
                js_string(sharing.get("version")),
                if t(sharing, "allowContinue") {
                    ", readers may continue it"
                } else {
                    ""
                }
            ),
            ("shared", None) => "Shared".into(),
            _ => "Private".into(),
        };
        rows.push(kv("Visibility", text));
    }
    if let Some(sharing) = sharing {
        rows.push(kv("Artifact", js_string(sharing.get("url"))));
    }
    let links = a(session, "artifacts");
    if !links.is_empty() {
        rows.push(kv(
            "Versions",
            links
                .iter()
                .map(|link| {
                    format!(
                        "v{} {} ({})",
                        js_string(link.get("version")),
                        js_string(link.get("id")),
                        js_string(link.get("status"))
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    if let Some(forked) = session.get("forkedFrom").filter(|forked| forked.is_object()) {
        let title = s(forked, "title")
            .filter(|title| !title.is_empty())
            .map(|title| format!("{title} "))
            .unwrap_or_default();
        rows.push(kv(
            "Forked from",
            format!(
                "{title}({}, version {})",
                js_string(forked.get("artifactId")),
                js_string(forked.get("version"))
            ),
        ));
    }
    rows
}

/// Markdown from Gateway, to stdout as it is or to `--out`; `--json`: where it went, or the text.
pub fn export_markdown(id: &str, path: &str, cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    if cmd.has("out") {
        return usage("--out needs a file name.", None);
    }
    let markdown = cmd.core.get_text(path)?;
    if let Some(out) = cmd.args.flag_set("out") {
        std::fs::write(cmd.ctx.cwd.join(out), &markdown).map_err(|error| {
            crate::error::Error::Cli(crate::error::CliError::new(format!("Could not write {out}: {error}")))
        })?;
        if cmd.json {
            return Ok(cmd.print(&json!({ "id": id, "path": out, "bytes": markdown.len() })));
        }
        cmd.out(format!("{} Wrote {out} ({} bytes).", style.ok, markdown.len()));
        return Ok(0);
    }
    if cmd.json {
        return Ok(cmd.print(&json!({ "id": id, "markdown": markdown })));
    }
    let text = if markdown.ends_with('\n') {
        markdown.clone()
    } else {
        format!("{markdown}\n")
    };
    if !cmd.ctx.io.write(&text) {
        cmd.out(markdown.strip_suffix('\n').unwrap_or(&markdown));
    }
    Ok(0)
}

fn short_title(title: &str) -> String {
    if js_len(title) > 40 {
        let units: Vec<u16> = title.encode_utf16().take(39).collect();
        format!("{}…", String::from_utf16_lossy(&units))
    } else {
        title.to_string()
    }
}

pub fn artifacts_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    let id = args.get(1).filter(|id| !id.is_empty());
    let need = || -> Result<String> {
        match id {
            Some(id) => Ok(id.clone()),
            None => usage(
                format!("Usage: nexus artifacts {action} <id>"),
                Some("`nexus artifacts` lists the artifacts shared with you and by you."),
            ),
        }
    };
    let path = |artifact: &str, suffix: &str| format!("/v1/artifacts/{}{suffix}", enc(artifact));

    if action == "help" || (cmd.has("help") && args.is_empty()) {
        if cmd.json {
            return Ok(
                cmd.print(&json!({ "usage": "nexus artifacts <command>", "commands": artifact_command_names() }))
            );
        }
        cmd.lines(artifacts_help(style));
        return Ok(0);
    }

    if action == "list" && cmd.has("external") {
        return external_list(cmd);
    }

    match action {
        "list" => {
            if ["audience", "author"]
                .iter()
                .any(|name| cmd.flag(name).is_some() || cmd.has(name))
            {
                return usage(
                    "--audience and --author go with --external.",
                    Some("nexus artifacts list --external --audience public"),
                );
            }
            if cmd.has("mine") && cmd.has("shared-with-me") {
                return usage("Use --mine or --shared-with-me, not both.", None);
            }
            if cmd.has("revoked") && cmd.has("all") {
                return usage("Use --revoked or --all, not both.", None);
            }
            let scope = if cmd.has("mine") {
                Some("shared_by_me")
            } else if cmd.has("shared-with-me") {
                Some("shared_with_me")
            } else {
                None
            };
            let status = if cmd.has("revoked") {
                Some("revoked")
            } else if cmd.has("all") {
                Some("all")
            } else {
                None
            };
            let query = search(&[
                ("scope", scope.map(String::from)),
                ("status", status.map(String::from)),
                ("q", cmd.flag_set_str("search")),
            ]);
            let page = cmd.core.get(&format!("/v1/artifacts{query}"))?;
            if cmd.json {
                return Ok(cmd.print(&page));
            }
            let artifacts = a(&page, "artifacts");
            if artifacts.is_empty() {
                cmd.out(format!(
                    "No artifacts yet. Share a chat session: {}",
                    style.cyan("nexus chat share <session> --org")
                ));
                return Ok(0);
            }
            let rows: Vec<Vec<String>> = artifacts
                .iter()
                .map(|artifact| {
                    vec![
                        js_string(artifact.get("id")),
                        short_title(&js_string(artifact.get("title"))),
                        author_text(artifact.get("author"), t(artifact, "mine")),
                        audience_text(artifact.get("audience").unwrap_or(&Value::Null)),
                        format!("v{}", js_string(artifact.get("version"))),
                        ago(s(artifact, "createdAt").unwrap_or("")),
                        if s(artifact, "status") == Some("revoked") {
                            style.dim("revoked")
                        } else {
                            "active".into()
                        },
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[
                    col("Artifact"),
                    col("Title"),
                    col("Author"),
                    col("Audience"),
                    right("Version"),
                    col("Shared"),
                    col("Status"),
                ],
                rows_of(rows),
                None,
            ));
            if s(&page, "nextCursor").is_some_and(|c| !c.is_empty()) {
                cmd.out(style.dim("More: the API pages with ?cursor=… (GET /v1/artifacts)."));
            }
            Ok(0)
        }
        "show" => {
            let detail = cmd.core.get(&path(&need()?, ""))?;
            if cmd.json {
                return Ok(cmd.print(&detail));
            }
            let did = js_string(detail.get("id"));
            let models = strings(&detail, "models");
            let outside = outside_text(&detail);
            let mut rows = vec![
                kv(
                    "Artifact",
                    format!(
                        "{did} (version {}, {})",
                        js_string(detail.get("version")),
                        js_string(detail.get("status"))
                    ),
                ),
                kv("Author", author_text(detail.get("author"), t(&detail, "mine"))),
            ];
            if let Some(organization) = detail.get("organization").filter(|value| truthy(value)) {
                rows.push(kv("Organization", js_string(Some(organization))));
            }
            rows.push(kv(
                "Audience",
                audience_text(detail.get("audience").unwrap_or(&Value::Null)),
            ));
            if let Some(outside) = &outside {
                rows.push(kv("Shared outside", outside.clone()));
                if detail.get("includeCode").is_some() {
                    rows.push(kv(
                        "Code",
                        if t(&detail, "includeCode") {
                            "Code sources included"
                        } else {
                            "Code sources left out"
                        },
                    ));
                }
            }
            rows.push(kv(
                "Mode",
                if s(&detail, "mode") == Some("ask") {
                    "code answers"
                } else {
                    "Chat"
                },
            ));
            rows.push(kv(
                "Models",
                if models.is_empty() {
                    js_string(detail.get("model"))
                } else {
                    models.join(", ")
                },
            ));
            if detail.get("costUsd").is_some() {
                rows.push(kv("Cost", usd(n(&detail, "costUsd"))));
            }
            rows.push(kv("Shared", js_string(detail.get("createdAt"))));
            rows.push(kv(
                "Continue",
                if t(&detail, "allowContinue") {
                    format!("allowed: {}", style.cyan(&format!("nexus artifacts continue {did}")))
                } else {
                    "not allowed".into()
                },
            ));
            rows.push(kv(
                "Open",
                match detail.get("publicUrl").filter(|value| !value.is_null()) {
                    Some(public) => js_string(Some(public)),
                    None => js_string(detail.get("url")),
                },
            ));
            cmd.out(style.bold(&js_string(detail.get("title"))));
            cmd.lines(pairs(style, &rows));
            print_messages(a(&detail, "messages"), cmd);
            let hidden = n(&detail, "hiddenSources");
            if hidden != 0.0 {
                let why = if s(&detail, "variant") == Some("external") {
                    "left out of what is shared outside the organization"
                } else {
                    "from repositories you cannot see"
                };
                cmd.out("");
                cmd.out(style.dim(&format!(
                    "{} source{} hidden: {why}.",
                    js_string(detail.get("hiddenSources")),
                    if hidden == 1.0 { " is" } else { "s are" }
                )));
            }
            Ok(0)
        }
        "export" => {
            let artifact = need()?;
            export_markdown(&artifact, &path(&artifact, ".md"), cmd)
        }
        "images" => {
            let named = need()?;
            let key = artifact_key(&named);
            if key.is_empty() {
                return usage(
                    format!("No artifact in {named}."),
                    Some("Give an artifact id (art_…), a public link's token, or the link."),
                );
            }
            // A public link's token reads the artifact and its images as anyone with the link does.
            let base = if key.starts_with("art_") {
                path(&key, "")
            } else {
                format!("/v1/public/artifacts/{}", enc(&key))
            };
            save_images(
                cmd,
                &key,
                &args[2..],
                || cmd.core.get(&base),
                |image| cmd.core.get(&format!("{base}/attachments/{}", enc(image))),
            )
        }
        "continue" => {
            let session = cmd.core.post(&path(&need()?, "/continue"), json!({}))?;
            let sid = js_string(session.get("id"));
            remember(cmd, Some(&sid));
            if cmd.json {
                return Ok(cmd.print(&session));
            }
            cmd.out(format!(
                "{} Continued {} in a new private session, {sid}.",
                style.ok,
                style.bold(&js_string(session.get("title")))
            ));
            cmd.lines(pairs(
                style,
                &[
                    kv("Next", format!("{} continues it", style.cyan("nexus chat \"…\""))),
                    kv("Open", ui_link(cmd.config, &format!("/sessions/{sid}"))),
                ],
            ));
            Ok(0)
        }
        "access" => {
            let artifact = need()?;
            let grant = emails(cmd.flag("grant"));
            let deny = emails(cmd.flag("deny"));
            let remove = emails(cmd.flag("remove"));
            for name in ["grant", "deny", "remove"] {
                if cmd.has(name) || (cmd.flag(name).is_some() && emails(cmd.flag(name)).is_empty()) {
                    return usage(format!("--{name} needs one or more emails, comma-separated."), None);
                }
            }
            let changing = !grant.is_empty() || !deny.is_empty() || !remove.is_empty();
            let access = if changing {
                let body = crate::json::obj(vec![
                    ("grant", (!grant.is_empty()).then(|| json!(grant))),
                    ("deny", (!deny.is_empty()).then(|| json!(deny))),
                    ("remove", (!remove.is_empty()).then(|| json!(remove))),
                ]);
                cmd.core.post(&path(&artifact, "/access"), body)?
            } else {
                cmd.core.get(&path(&artifact, "/access"))?
            };
            if cmd.json {
                return Ok(cmd.print(&access));
            }
            if changing {
                cmd.out(format!(
                    "{} Updated access to {}: granted {}, denied {}, removed {}.",
                    style.ok,
                    js_string(access.get("id")),
                    grant.len(),
                    deny.len(),
                    remove.len()
                ));
            }
            print_access(&access, cmd);
            Ok(0)
        }
        "request" => {
            let artifact = need()?;
            if cmd.has("message") {
                return usage("--message needs a text.", None);
            }
            let body = match cmd.flag_set_str("message") {
                Some(message) => json!({ "message": message }),
                None => json!({}),
            };
            let answer = cmd.core.post(&path(&artifact, "/access-requests"), body)?;
            if cmd.json {
                return Ok(cmd.print(&answer));
            }
            if s(&answer, "status") == Some("granted") {
                cmd.out(format!(
                    "{} You already have access to {artifact}: {}",
                    style.ok,
                    style.cyan(&format!("nexus artifacts show {artifact}"))
                ));
            } else {
                cmd.out(format!(
                    "{} Asked for access to {artifact}; its author sees your request and can grant it.",
                    style.ok
                ));
            }
            Ok(0)
        }
        "revoke" if args.len() > 2 => {
            let ids: Vec<String> = args[1..].to_vec();
            let result = cmd.core.post("/v1/artifacts/revoke", json!({ "ids": ids }))?;
            if cmd.json {
                return Ok(cmd.print(&result));
            }
            let revoked = a(&result, "revoked");
            let skipped = a(&result, "skipped");
            for item in revoked {
                cmd.out(format!(
                    "{} Revoked {} (version {}).",
                    style.ok,
                    js_string(item.get("id")),
                    js_string(item.get("version"))
                ));
            }
            for item in skipped {
                cmd.out(format!(
                    "{} {}: {}",
                    style.yellow("Skipped"),
                    js_string(item.get("id")),
                    js_string(item.get("reason"))
                ));
            }
            cmd.out(format!(
                "{} revoked, {} skipped: nobody else can open the revoked ones any more.",
                revoked.len(),
                skipped.len()
            ));
            Ok(if skipped.is_empty() { 0 } else { 1 })
        }
        "revoke" => {
            let revoked = cmd.core.delete(&path(&need()?, ""))?;
            if cmd.json {
                return Ok(cmd.print(&revoked));
            }
            cmd.out(format!(
                "{} Revoked {} (version {}): nobody else can open it any more.",
                style.ok,
                js_string(revoked.get("id")),
                js_string(revoked.get("version"))
            ));
            Ok(0)
        }
        other => usage(
            format!("Unknown command `nexus artifacts {other}`."),
            Some("Use list, show, export, images, continue, access, request or revoke."),
        ),
    }
}

/// An artifact id, a public link's token, or a link to either (`…/a/<key>`, `…/artifacts/art_…`) (`artifactKey` in
/// artifacts.ts): a link's last path segment, else the argument as it is.
pub fn artifact_key(named: &str) -> String {
    let Some((_, rest)) = named.split_once("://") else {
        return named.to_string();
    };
    let address = rest.split(['?', '#']).next().unwrap_or("");
    let path = address.split_once('/').map(|(_, path)| path).unwrap_or("");
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Who has an artifact shared outside, who asked, how often each opened it.
fn print_access(access: &Value, cmd: &Cmd) {
    let style = cmd.style;
    let audience = s(access, "audience").unwrap_or("");
    let mut rows = vec![
        kv(
            "Artifact",
            format!("{} ({})", js_string(access.get("id")), js_string(access.get("status"))),
        ),
        kv(
            "Audience",
            match audience {
                "public" => "anyone with the link".to_string(),
                "external" => "named people outside the organization".to_string(),
                _ => js_string(access.get("audience")),
            },
        ),
    ];
    if let Some(link) = access.get("publicUrl").filter(|value| truthy(value)) {
        rows.push(kv("Link", js_string(Some(link))));
    }
    if audience == "public" {
        rows.push(kv(
            "Expires",
            match access.get("expiresAt").filter(|value| !value.is_null()) {
                Some(at) => js_string(Some(at)),
                None => "never".into(),
            },
        ));
    }
    rows.push(kv(
        "Opens",
        format!(
            "{}{}",
            js_string(access.get("opens")),
            match s(access, "lastOpenedAt").filter(|at| !at.is_empty()) {
                Some(at) => format!(" (last {})", ago(at)),
                None => String::new(),
            }
        ),
    ));
    rows.push(kv("Reports", js_string(access.get("reports"))));
    cmd.lines(pairs(style, &rows));
    if audience == "public" {
        return;
    }
    let people = a(access, "people");
    if people.is_empty() {
        cmd.out("");
        cmd.out("Nobody named yet.");
        return;
    }
    let when = |person: &Value, key: &str| match s(person, key).filter(|at| !at.is_empty()) {
        Some(at) => ago(at),
        None => "–".into(),
    };
    let rows: Vec<Vec<String>> = people
        .iter()
        .map(|person| {
            vec![
                js_string(person.get("email")),
                if s(person, "status") == Some("requested") {
                    style.yellow("requested")
                } else {
                    js_string(person.get("status"))
                },
                js_string(person.get("opens")),
                when(person, "lastOpenedAt"),
                when(person, "requestedAt"),
            ]
        })
        .collect();
    cmd.out("");
    cmd.lines(list(
        style,
        &[
            col("Person"),
            col("Status"),
            right("Opens"),
            col("Last opened"),
            col("Asked"),
        ],
        rows_of(rows),
        None,
    ));
    if people.iter().any(|person| s(person, "status") == Some("requested")) {
        cmd.out(style.dim(&format!(
            "Grant: nexus artifacts access {} --grant <email>",
            js_string(access.get("id"))
        )));
    }
}

/// `nexus artifacts list --external`: the admin view of everything shared outside the organization.
fn external_list(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    if cmd.has("mine") || cmd.has("shared-with-me") {
        return usage(
            "--external lists everything shared outside; leave out --mine and --shared-with-me.",
            None,
        );
    }
    if cmd.has("revoked") && cmd.has("all") {
        return usage("Use --revoked or --all, not both.", None);
    }
    if let Some(audience) = cmd.flag("audience")
        && audience != "external"
        && audience != "public"
    {
        return usage(
            "--audience must be external (named people) or public (anyone with the link).",
            None,
        );
    }
    for name in ["audience", "author", "team", "search"] {
        if cmd.has(name) {
            return usage(format!("--{name} needs a value."), None);
        }
    }
    let status = if cmd.has("revoked") {
        Some("revoked".to_string())
    } else if cmd.has("all") {
        Some("all".to_string())
    } else {
        None
    };
    let query = search(&[
        ("audience", cmd.flag_set_str("audience")),
        ("author", cmd.flag_set_str("author")),
        ("team", cmd.flag_set_str("team")),
        ("status", status),
        ("q", cmd.flag_set_str("search")),
    ]);
    let page = cmd.core.get(&format!("/v1/artifacts/external{query}"))?;
    if cmd.json {
        return Ok(cmd.print(&page));
    }
    let artifacts = a(&page, "artifacts");
    if artifacts.is_empty() {
        cmd.out("Nothing is shared outside your organization.");
        return Ok(0);
    }
    let count = |artifact: &Value, key: &str| js_string(artifact.get(key));
    let rows: Vec<Vec<String>> = artifacts
        .iter()
        .map(|artifact| {
            let audience = artifact.get("audience").unwrap_or(&Value::Null);
            vec![
                js_string(artifact.get("id")),
                short_title(&js_string(artifact.get("title"))),
                author_text(artifact.get("author"), t(artifact, "mine")),
                if s(audience, "kind") == Some("public") {
                    "public".into()
                } else {
                    "named".into()
                },
                count(artifact, "opens"),
                count(artifact, "readers"),
                if n(artifact, "requests") != 0.0 {
                    style.yellow(&count(artifact, "requests"))
                } else {
                    "0".into()
                },
                if n(artifact, "reports") != 0.0 {
                    style.red(&count(artifact, "reports"))
                } else {
                    "0".into()
                },
                day(s(artifact, "expiresAt")),
                if s(artifact, "status") == Some("revoked") {
                    style.dim("revoked")
                } else {
                    "active".into()
                },
            ]
        })
        .collect();
    cmd.lines(list(
        style,
        &[
            col("Artifact"),
            col("Title"),
            col("Author"),
            col("Audience"),
            right("Opens"),
            right("Readers"),
            right("Requests"),
            right("Reports"),
            col("Expires"),
            col("Status"),
        ],
        rows_of(rows),
        None,
    ));
    if let Some(totals) = page.get("totals").filter(|totals| totals.is_object()) {
        cmd.out(style.dim(&format!(
            "{} active: {} named, {} public.",
            js_string(totals.get("active")),
            js_string(totals.get("named")),
            js_string(totals.get("public"))
        )));
    }
    if s(&page, "nextCursor").is_some_and(|c| !c.is_empty()) {
        cmd.out(style.dim("More: the API pages with ?cursor=… (GET /v1/artifacts/external)."));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_questions_images_as_claude_code_shows_them() {
        let attachments = serde_json::json!([
            { "id": "cha_1", "type": "image", "mediaType": "image/png", "bytes": 319_488, "width": 1600, "height": 1000, "name": "shot.png" },
            { "id": "cha_2", "type": "image", "mediaType": "image/jpeg", "bytes": 3_145_728 },
            { "id": "cha_3", "type": "image", "mediaType": "image/gif", "bytes": 10 },
        ]);
        assert_eq!(
            image_lines(Some(&attachments)),
            vec![
                "[Image #1] shot.png (1600 × 1000, 312 KB)",
                "[Image #2] image/jpeg (3.0 MB)",
                "[Image #3] image/gif (1 KB)",
            ]
        );
        assert!(image_lines(None).is_empty());
    }

    #[test]
    fn an_artifact_is_named_by_its_id_its_token_or_a_link() {
        assert_eq!(artifact_key("art_1"), "art_1");
        assert_eq!(artifact_key("tok_public"), "tok_public");
        assert_eq!(artifact_key("https://nexus.test/a/tok_public?ref=x#top"), "tok_public");
        assert_eq!(artifact_key("http://ui.test/app/acme/artifacts/art_1/"), "art_1");
        assert_eq!(artifact_key("https://nexus.test"), "");
    }

    #[test]
    fn describes_audiences_and_authors_in_words() {
        assert_eq!(audience_text(&json!({ "kind": "org" })), "your organization");
        assert_eq!(audience_text(&json!({ "kind": "teams", "teams": ["web"] })), "team web");
        assert_eq!(
            audience_text(&json!({ "kind": "teams", "teams": ["web", "api"] })),
            "teams web, api"
        );
        assert_eq!(
            audience_text(&json!({ "kind": "people", "people": ["a@x", "b@y"] })),
            "a@x, b@y"
        );
        let ada = json!({ "email": "ada@acme.test", "name": "Ada" });
        assert_eq!(author_text(Some(&ada), false), "Ada <ada@acme.test>");
        assert_eq!(author_text(Some(&ada), true), "you");
        assert_eq!(author_text(None, false), "–");
    }

    #[test]
    fn links_into_the_organization() {
        let mut config = Config::with_url("http://core.test");
        config.ui_url = Some("http://ui.test/app".into());
        assert_eq!(
            ui_link(&config, "/sessions/chs_1"),
            "http://ui.test/app?next=%2Fsessions%2Fchs_1"
        );
        config.extra.insert("org".into(), json!("acme"));
        assert_eq!(
            ui_link(&config, "/sessions/chs_1"),
            "http://ui.test/app/acme/sessions/chs_1"
        );
    }
}
