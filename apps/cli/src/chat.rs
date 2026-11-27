//! `nexus chat` (`chat.ts`): the same chat sessions as Nexus UI's home, from a terminal. A prompt continues the
//! session this CLI used last (kept in its config) unless `--new` or `--session` say otherwise; the answer streams.
//! `--image` attaches images (Rust gateway): read here, checked for count and size before anything is sent, and shown
//! as `[Image #1] …` lines, as Claude Code shows a pasted image. `nexus chat sessions images` saves them to files
//! (`images.rs`).

use std::path::Path;

use base64::Engine as _;
use nexus_client::base::images::{MAX_IMAGE_BYTES, MAX_IMAGES, MAX_TOTAL_BYTES};
use serde_json::{Value, json};

use crate::artifacts::{
    audience_text, export_markdown, image_lines, print_messages, print_sources, state_rows, ui_link,
};
use crate::catalog::{CHAT_COMMANDS, CHAT_SHARE_COMMANDS};
use crate::cli::Cmd;
use crate::client::{enc, search};
use crate::config::{load_config, save_config};
use crate::error::{CliError, Error, Result, budget_hint, usage};
use crate::images::save_images;
use crate::json::{a, js_string, n, obj, s, t};
use crate::style::{Row, Style, ago, col, flex, js_len, kv, legend, list, pad_end, pairs, quote, right, usd};
use crate::teams::js_integer;

pub fn chat_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{}  nexus chat \"prompt\" [--image a.png,b.png] [--session <id> | --new] [--model m] [--team T] [--no-web] [--json]",
            style.bold("Usage")
        ),
        "       nexus chat sessions <command>".to_string(),
        String::new(),
        "A prompt continues the session you used last; --new starts another. --image attaches PNG, JPEG, GIF or WebP files".to_string(),
        "(up to 8, 5 MB each). Sessions are private to you. The model may search the web when your organization allows".to_string(),
        "it; --no-web turns that off for the session.".to_string(),
        String::new(),
        style.bold("Sessions"),
    ];
    lines.extend(
        CHAT_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 46)))),
    );
    lines.push(String::new());
    lines.push(style.bold("Sharing"));
    for (name, text) in CHAT_SHARE_COMMANDS {
        lines.push(format!("  {}", style.cyan(name)));
        lines.push(format!("      {text}"));
    }
    lines
}

/// Keep (or forget) the session `nexus chat` continues, without touching the rest of the config.
fn remember(cmd: &Cmd, session: Option<&str>) {
    let mut current = load_config(&cmd.ctx.config_path, &cmd.ctx.env_fn());
    if current.chat_session.as_deref() == session {
        return;
    }
    current.chat_session = session.map(String::from);
    let _ = save_config(&cmd.ctx.config_path, &current);
}

pub fn chat_command(args: &[String], cmd: &Cmd, last_session: Option<&str>) -> Result<i32> {
    // `--help` wins at every depth: `nexus chat share --help` prints help, it shares nothing.
    if args.first().map(String::as_str) == Some("help") || cmd.has("help") {
        if cmd.json {
            return Ok(cmd.print(&json!({
                "usage": "nexus chat \"prompt\" | nexus chat sessions <command>",
                "commands": ["sessions list", "sessions show", "sessions images", "sessions rename", "sessions archive", "sessions delete", "share", "export"],
            })));
        }
        cmd.lines(chat_help(cmd.style));
        return Ok(0);
    }
    if args.first().map(String::as_str) == Some("sessions") {
        return sessions_command(&args[1..], cmd, last_session);
    }
    if args.first().map(String::as_str) == Some("share") {
        return share(&args[1..], cmd);
    }
    if args.first().map(String::as_str) == Some("export") {
        let Some(id) = args.get(1).filter(|id| !id.is_empty()) else {
            return usage(
                "Usage: nexus chat export <session> [--md] [--out file]",
                Some("`nexus chat sessions` lists your sessions and their ids."),
            );
        };
        return export_markdown(id, &format!("/v1/chat/sessions/{}/export.md", enc(id)), cmd);
    }
    let prompt = args.join(" ").trim().to_string();
    if prompt.is_empty() && cmd.args.flag("image").is_none() {
        return usage(
            "Usage: nexus chat \"prompt\" [--image a.png,b.png] [--session <id> | --new] [--model m] [--team T] [--no-web] [--json]",
            Some("The prompt is one argument; quote it. `nexus chat sessions` lists your sessions."),
        );
    }
    ask(&prompt, cmd, last_session)
}

const MB: f64 = 1024.0 * 1024.0;

/// `--image a.png,b.jpg` (`readImages` in chat.ts): each file as base64 with its name, checked for count and size
/// before anything is sent. Relative paths are the working directory's.
pub fn read_images(spec: Option<&str>, cwd: &Path) -> Result<Vec<Value>> {
    let Some(spec) = spec else {
        return Ok(Vec::new());
    };
    let paths: Vec<&str> = spec.split(',').map(str::trim).filter(|path| !path.is_empty()).collect();
    if paths.is_empty() {
        return usage(
            "--image needs a file.",
            Some("Give the path of a PNG, JPEG, GIF or WebP image: --image shot.png"),
        );
    }
    if paths.len() > MAX_IMAGES {
        return usage(format!("At most {MAX_IMAGES} images go with one message."), None);
    }
    let mut total = 0usize;
    let mut images = Vec::with_capacity(paths.len());
    for path in paths {
        let Ok(bytes) = std::fs::read(cwd.join(path)) else {
            return usage(
                format!("Cannot read {path}."),
                Some("Give the path of a PNG, JPEG, GIF or WebP image."),
            );
        };
        if bytes.len() > MAX_IMAGE_BYTES {
            return usage(
                format!(
                    "{path} is {:.1} MB; an image may be at most {} MB.",
                    bytes.len() as f64 / MB,
                    MAX_IMAGE_BYTES >> 20
                ),
                Some("Send a smaller one: one window instead of the screen, or a lower resolution."),
            );
        }
        total += bytes.len();
        let name = Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        images.push(json!({ "data": base64::engine::general_purpose::STANDARD.encode(&bytes), "name": name }));
    }
    if total as i64 > MAX_TOTAL_BYTES {
        return usage(
            format!(
                "The images total {:.1} MB; one message takes at most {} MB.",
                total as f64 / MB,
                MAX_TOTAL_BYTES >> 20
            ),
            None,
        );
    }
    Ok(images)
}

fn ask(prompt: &str, cmd: &Cmd, last_session: Option<&str>) -> Result<i32> {
    let style = cmd.style;
    let named = cmd.args.flag_set("session");
    if named.is_some() && cmd.has("new") {
        return usage("Use --session or --new, not both.", None);
    }
    // Web search is on by default (Gateway's default); --no-web turns the session's switch off.
    let no_web = cmd.has("no-web");
    // The model named, else the default `nexus models pick` saved, else the session's own.
    let model = cmd.flag("model").or(cmd.config.chat_model()).map(|m| {
        if m == "auto" || m == "default" {
            format!("nexus/{m}")
        } else {
            m.to_string()
        }
    });
    let team = cmd.flag("team").map(String::from).or_else(|| cmd.ctx.var("NEXUS_TEAM"));
    // `flag`, not `flag_set`: an empty `--image=` (an unset `"$IMG"`) is refused as in chat.ts, never dropped.
    let images = read_images(cmd.args.flag("image"), &cmd.ctx.cwd)?;
    let options = |content: Option<&str>| {
        obj(vec![
            ("content", content.map(|c| json!(c))),
            (
                "images",
                content
                    .filter(|_| !images.is_empty())
                    .map(|_| Value::Array(images.clone())),
            ),
            ("model", model.as_ref().map(|m| json!(m))),
            ("team", team.as_ref().filter(|t| !t.is_empty()).map(|t| json!(t))),
        ])
    };

    // Which session: the one named, else the one used last (if it is still yours here), else a new one.
    let wanted = named.map(String::from).or_else(|| {
        if cmd.has("new") {
            None
        } else {
            last_session.map(String::from)
        }
    });
    let mut session: Option<Value> = None;
    if let Some(wanted) = wanted.filter(|w| !w.is_empty()) {
        match cmd.core.get(&format!("/v1/chat/sessions/{}", enc(&wanted))) {
            Ok(found) => session = Some(found),
            Err(Error::Core(error)) if error.status == 404 && named.is_none() => {}
            Err(error) => return Err(error),
        }
    }
    let session = match session {
        Some(session) if no_web && session.get("webSearch").and_then(Value::as_bool).unwrap_or(false) => {
            let path = format!("/v1/chat/sessions/{}", enc(&js_string(session.get("id"))));
            cmd.core.patch(&path, json!({ "webSearch": false }))?
        }
        Some(session) => session,
        None => {
            let mut body = options(None);
            if let (true, Value::Object(map)) = (no_web, &mut body) {
                map.insert("webSearch".into(), json!(false));
            }
            cmd.core.post("/v1/chat/sessions", body)?
        }
    };
    let id = js_string(session.get("id"));
    remember(cmd, Some(&id));
    let body = options(Some(prompt));

    let refused = |error: Error| session_refusal(error, &id);

    if cmd.json {
        let mut body = body;
        if let Value::Object(map) = &mut body {
            map.insert("stream".into(), json!(false));
        }
        let result = cmd
            .core
            .post(&format!("/v1/chat/sessions/{}/messages", enc(&id)), body)
            .map_err(refused)?;
        return Ok(cmd.print(&result));
    }
    let mut streamed = false;
    let io = &cmd.ctx.io;
    let (_, _, message) = cmd
        .core
        .stream_chat_message(&id, body, &mut |chunk| {
            // The stored question comes first: its images, before the answer.
            let shown = image_lines(chunk.pointer("/nexus/question/attachments"));
            if !shown.is_empty() {
                for line in &shown {
                    cmd.out(style.dim(line));
                }
                cmd.out("");
            }
            let delta = chunk
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .filter(|d| !d.is_empty());
            if let Some(delta) = delta
                && io.write(delta)
            {
                streamed = true;
            }
        })
        .map_err(refused)?;
    let content = js_string(message.get("content"));
    if streamed {
        io.write(if content.ends_with('\n') { "" } else { "\n" });
    } else {
        cmd.out(&content);
    }
    print_sources(&message, cmd);
    cmd.out("");
    cmd.out(style.dim(&footer(&message, &id)));
    Ok(0)
}

/// A refusal Gateway kept in the session names that session; a budget refusal also says who can raise the budget.
pub(crate) fn session_refusal(error: Error, id: &str) -> Error {
    match &error {
        // An unpaid month, a failed charge or a month's usage limit is not the model's fault: another model is refused
        // too, so its own hint applies.
        Error::Core(core)
            if core
                .details
                .as_ref()
                .and_then(|d| d.get("session"))
                .is_some_and(crate::json::truthy)
                && !matches!(
                    core.code.as_deref(),
                    Some(
                        "payment_past_due"
                            | "payment_failed"
                            | "usage_limit_reached"
                            | "credit_exhausted"
                            | "charge_processing"
                    )
                ) =>
        {
            let kept = format!(
                "The refusal is kept in session {id}; `nexus chat sessions show {id}`. Try another --model, or `nexus models`."
            );
            let hint = if core.code.as_deref() == Some("budget_exceeded") {
                format!("{kept} {}", budget_hint(core))
            } else {
                kept
            };
            Error::Cli(CliError::hint(core.message.clone(), hint))
        }
        _ => error,
    }
}

fn footer(message: &Value, session: &str) -> String {
    let model = s(message, "model").filter(|m| !m.is_empty());
    let requested = match (s(message, "requestedModel"), model) {
        (Some(requested), Some(_)) if requested.starts_with("nexus/") => format!(" ({requested})"),
        _ => String::new(),
    };
    format!(
        "{}{requested} · {} · session {session}",
        s(message, "model").unwrap_or("no model called"),
        usd(n(message, "costUsd"))
    )
}

fn sessions_command(args: &[String], cmd: &Cmd, last_session: Option<&str>) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    let id = args.get(1).filter(|id| !id.is_empty());
    let rest: Vec<String> = args.iter().skip(2).cloned().collect();
    let need = || -> Result<String> {
        match id {
            Some(id) => Ok(id.clone()),
            None => usage(
                format!("Usage: nexus chat sessions {action} <id>"),
                Some("`nexus chat sessions` lists your sessions and their ids."),
            ),
        }
    };

    match action {
        "list" => {
            let limit = match cmd.args.flag_set("limit") {
                None => None,
                Some(raw) => match js_integer(raw) {
                    Some(limit) if (1.0..=100.0).contains(&limit) => Some(limit),
                    _ => return usage("--limit must be a whole number from 1 to 100.", None),
                },
            };
            let query = search(&[
                ("limit", limit.map(crate::style::js_number)),
                ("q", cmd.args.flag_set("search").map(String::from)),
                (
                    "archived",
                    Some(if cmd.has("all") { "include" } else { "exclude" }.to_string()),
                ),
            ]);
            let page = cmd.core.get(&format!("/v1/chat/sessions{query}"))?;
            if cmd.json {
                return Ok(cmd.print(&page));
            }
            let sessions = a(&page, "sessions");
            if sessions.is_empty() {
                cmd.out(match cmd.args.flag_set("search") {
                    Some(words) => format!("No sessions match {}.", quote(words)),
                    None => format!(
                        "No chat sessions yet. Start one: {}",
                        style.cyan("nexus chat \"hello\"")
                    ),
                });
                return Ok(0);
            }
            let rows: Vec<Row> = sessions
                .iter()
                .map(|session| {
                    let sid = js_string(session.get("id"));
                    let title = js_string(session.get("title"));
                    let short = if js_len(&title) > 40 {
                        let units: Vec<u16> = title.encode_utf16().take(39).collect();
                        format!("{}…", String::from_utf16_lossy(&units))
                    } else {
                        title
                    };
                    let selected = Some(sid.as_str()) == last_session;
                    let cells = vec![
                        sid,
                        format!(
                            "{short}{}",
                            if t(session, "archived") {
                                style.dim(" (archived)")
                            } else {
                                String::new()
                            }
                        ),
                        if s(session, "mode") == Some("ask") {
                            "ask your code".into()
                        } else {
                            "chat".into()
                        },
                        s(session, "lastModel")
                            .map(String::from)
                            .unwrap_or_else(|| js_string(session.get("model"))),
                        js_string(session.get("messageCount")),
                        usd(n(session, "costUsd")),
                        ago(s(session, "updatedAt").unwrap_or("")),
                    ];
                    Row {
                        cells,
                        selected,
                        muted: t(session, "archived"),
                        ..Row::default()
                    }
                })
                .collect();
            cmd.lines(list(
                style,
                &[
                    col("Session"),
                    flex("Title"),
                    col("Mode"),
                    col("Model"),
                    right("Messages"),
                    right("Cost"),
                    col("Active"),
                ],
                rows,
                None,
            ));
            if let Some(last) = last_session
                && sessions.iter().any(|session| s(session, "id") == Some(last))
            {
                cmd.out("");
                cmd.out(legend(
                    style,
                    &[(
                        style.select.clone(),
                        "the session `nexus chat` continues; --new starts another",
                    )],
                ));
            }
            if s(&page, "nextCursor").is_some_and(|c| !c.is_empty()) {
                cmd.out(style.dim("More: raise --limit, or --search to narrow."));
            }
            Ok(0)
        }
        "show" => {
            let detail = cmd.core.get(&format!("/v1/chat/sessions/{}", enc(&need()?)))?;
            if cmd.json {
                return Ok(cmd.print(&detail));
            }
            let model = js_string(detail.get("model"));
            let mut entries = vec![
                kv("Session", js_string(detail.get("id"))),
                kv(
                    "Mode",
                    if s(&detail, "mode") == Some("ask") {
                        "code answers"
                    } else {
                        "Chat"
                    },
                ),
                kv(
                    "Model",
                    match s(&detail, "lastModel").filter(|m| !m.is_empty()) {
                        Some(last) => format!(
                            "{last}{}",
                            if model != last {
                                format!(" ({model})")
                            } else {
                                String::new()
                            }
                        ),
                        None => model,
                    },
                ),
            ];
            if let Some(team) = s(&detail, "team").filter(|t| !t.is_empty()) {
                entries.push(kv("Team", team));
            }
            entries.push(kv("Cost", usd(n(&detail, "costUsd"))));
            entries.extend(state_rows(&detail));
            // `uiLink` in config.ts: in the signed-in organization when the config names it.
            entries.push(kv(
                "Open",
                ui_link(cmd.config, &format!("/sessions/{}", js_string(detail.get("id")))),
            ));
            cmd.out(style.bold(&js_string(detail.get("title"))));
            cmd.lines(pairs(style, &entries));
            print_messages(a(&detail, "messages"), cmd);
            Ok(0)
        }
        "images" => {
            let named = need()?;
            let session = format!("/v1/chat/sessions/{}", enc(&named));
            save_images(
                cmd,
                &named,
                &rest,
                || cmd.core.get(&session),
                |image| cmd.core.get(&format!("{session}/attachments/{}", enc(image))),
            )
        }
        "rename" => {
            let title = rest.join(" ").trim().to_string();
            if title.is_empty() {
                return usage("Usage: nexus chat sessions rename <id> <title…>", None);
            }
            let session = cmd.core.patch(
                &format!("/v1/chat/sessions/{}", enc(&need()?)),
                json!({ "title": title }),
            )?;
            if cmd.json {
                return Ok(cmd.print(&session));
            }
            cmd.out(format!(
                "{} Renamed {} to {}.",
                style.ok,
                js_string(session.get("id")),
                style.bold(&js_string(session.get("title")))
            ));
            Ok(0)
        }
        "archive" => {
            let undo = cmd.has("undo");
            let session = cmd.core.patch(
                &format!("/v1/chat/sessions/{}", enc(&need()?)),
                json!({ "archived": !undo }),
            )?;
            if cmd.json {
                return Ok(cmd.print(&session));
            }
            let sid = js_string(session.get("id"));
            cmd.out(format!(
                "{} {} {sid}{}.",
                style.ok,
                if undo { "Restored" } else { "Archived" },
                if undo {
                    String::new()
                } else {
                    format!(
                        "; {} brings it back",
                        style.cyan(&format!("nexus chat sessions archive {sid} --undo"))
                    )
                }
            ));
            Ok(0)
        }
        "delete" => {
            let removed = cmd.core.delete(&format!("/v1/chat/sessions/{}", enc(&need()?)))?;
            let rid = js_string(removed.get("id"));
            if Some(rid.as_str()) == last_session {
                remember(cmd, None);
            }
            if cmd.json {
                return Ok(cmd.print(&removed));
            }
            let count = n(&removed, "messages");
            cmd.out(format!(
                "{} Deleted {rid} and its {} message{}.",
                style.ok,
                js_string(removed.get("messages")),
                if count == 1.0 { "" } else { "s" }
            ));
            Ok(0)
        }
        other => usage(
            format!("Unknown command `nexus chat sessions {other}`."),
            Some("Use list, show, images, rename, archive or delete."),
        ),
    }
}

/// `nexus chat share`: exactly one audience; teams and people comma-separated.
/// Exactly one audience: --private (stop sharing: `None`), --org, --team T, --people a@x,b@y, --to a@x,b@y or --public
/// [--expires 30d]; teams and people comma-separated (`shareAudience` in `chat.ts`). `others` are the command's own
/// options, which --private refuses. `nexus chat share` and `nexus agents share` read their audience with it.
pub(crate) fn share_audience(cmd: &Cmd, usage_text: &str, others: &[&str]) -> Result<Option<Value>> {
    let team = cmd.flag("team").is_some() || cmd.has("team");
    let people = cmd.flag("people").is_some() || cmd.has("people");
    let to = cmd.flag("to").is_some() || cmd.has("to");
    let public = cmd.has("public");
    let private = cmd.has("private");
    let chosen = [private, cmd.has("org"), team, people, to, public]
        .iter()
        .filter(|chosen| **chosen)
        .count();
    if chosen != 1 {
        return usage(
            "Choose who may read it: --private, --org, --team T, --people a@x,b@y, --to a@x,b@y or --public (one of them).",
            Some(usage_text),
        );
    }
    if private {
        if ["expires", "replace"]
            .iter()
            .chain(others)
            .any(|flag| cmd.has(flag) || cmd.flag(flag).is_some())
        {
            return usage("--private takes no other option: it stops sharing.", Some(usage_text));
        }
        return Ok(None);
    }
    let expires = cmd.flag("expires").is_some() || cmd.has("expires");
    if expires && !public {
        return usage("--expires is for --public links.", Some(usage_text));
    }
    let days = cmd.flag("expires").unwrap_or("");
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let duration_ok =
        digits(days) || ((days.ends_with('d') || days.ends_with('h')) && digits(&days[..days.len().saturating_sub(1)]));
    if expires && !duration_ok {
        return usage("--expires needs a duration: 30d, 12h or a number of days.", None);
    }
    let list = |value: Option<&str>| -> Vec<String> {
        value
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(String::from)
            .collect()
    };
    let teams = list(cmd.flag("team"));
    let emails = list(cmd.flag("people"));
    let outsiders = list(cmd.flag("to"));
    if team && teams.is_empty() {
        return usage("--team needs one or more team slugs, comma-separated.", None);
    }
    if people && emails.is_empty() {
        return usage("--people needs one or more emails, comma-separated.", None);
    }
    if to && outsiders.is_empty() {
        return usage("--to needs one or more emails, comma-separated.", None);
    }
    let audience = if cmd.has("org") {
        "org"
    } else if team {
        "teams"
    } else if people {
        "people"
    } else if to {
        "external"
    } else {
        "public"
    };
    let expires_in = expires.then(|| match days.parse::<u64>() {
        Ok(number) if digits(days) => json!(number),
        _ => json!(days),
    });
    Ok(Some(obj(vec![
        ("audience", Some(json!(audience))),
        ("teams", team.then(|| json!(teams))),
        (
            "people",
            if people {
                Some(json!(emails))
            } else if to {
                Some(json!(outsiders))
            } else {
                None
            },
        ),
        ("expiresIn", expires_in),
        ("replace", cmd.has("replace").then(|| json!(true))),
    ])))
}

fn share(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let usage_text = "Usage: nexus chat share <session> [--private | --org | --team T | --people a@x,b@y | --to a@x,b@y | --public [--expires 30d]] [--replace] [--include-code] [--allow-continue] [--json]";
    let Some(id) = args.first().filter(|id| !id.is_empty()) else {
        return usage(
            usage_text,
            Some("`nexus chat sessions` lists your sessions and their ids."),
        );
    };
    let Some(mut body) = share_audience(cmd, usage_text, &["include-code", "allow-continue"])? else {
        return unshare(id, cmd);
    };
    let outside = matches!(s(&body, "audience"), Some("external" | "public"));
    if cmd.has("include-code") && !outside {
        return usage(
            "--include-code is for shares outside your organization (--to or --public).",
            Some(usage_text),
        );
    }
    if let Some(fields) = body.as_object_mut() {
        fields.insert("allowContinue".into(), json!(cmd.has("allow-continue")));
        if outside {
            fields.insert("includeCode".into(), json!(cmd.has("include-code")));
        }
    }
    let artifact = cmd.core.post(&format!("/v1/chat/sessions/{}/share", enc(id)), body)?;
    if cmd.json {
        return Ok(cmd.print(&artifact));
    }
    let aid = js_string(artifact.get("id"));
    let audience = artifact.get("audience").unwrap_or(&Value::Null);
    let kind = s(audience, "kind").unwrap_or("");
    let whom = match kind {
        "external" => format!(
            "outside your organization with {}",
            a(audience, "people")
                .iter()
                .map(|person| js_string(Some(person)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        "public" => "with anyone with the link".into(),
        _ => format!("with {}", audience_text(audience)),
    };
    cmd.out(format!(
        "{} Shared {} {whom} (version {}).",
        style.ok,
        style.bold(&js_string(artifact.get("title"))),
        js_string(artifact.get("version"))
    ));
    let is_outside = kind == "external" || kind == "public";
    let mut rows = vec![
        kv("Artifact", aid.clone()),
        kv(
            "Open",
            match artifact.get("publicUrl").filter(|value| !value.is_null()) {
                Some(public) => js_string(Some(public)),
                None => js_string(artifact.get("url")),
            },
        ),
    ];
    if kind == "public" {
        rows.push(kv(
            "Expires",
            match artifact.get("expiresAt").filter(|value| !value.is_null()) {
                Some(at) => js_string(Some(at)),
                None => "never".into(),
            },
        ));
    }
    if kind == "external" {
        rows.push(kv(
            "Sign-in",
            "readers open it signed in with that email; anyone else can ask you for access",
        ));
    }
    if is_outside {
        rows.push(kv(
            "Code",
            if t(&artifact, "includeCode") {
                "Code sources included"
            } else {
                "Code sources left out"
            },
        ));
    }
    rows.push(kv("Markdown", js_string(artifact.get("markdownUrl"))));
    rows.push(kv(
        "Readers",
        if t(&artifact, "allowContinue") {
            "may continue it in a private chat of their own"
        } else {
            "can read it (--allow-continue lets them continue it)"
        },
    ));
    if kind == "external" {
        rows.push(kv("Access", style.cyan(&format!("nexus artifacts access {aid}"))));
    }
    rows.push(kv("Revoke", style.cyan(&format!("nexus artifacts revoke {aid}"))));
    cmd.lines(pairs(style, &rows));
    Ok(0)
}

/// `nexus chat share <session> --private`: only you again, every version revoked.
fn unshare(id: &str, cmd: &Cmd) -> Result<i32> {
    let result = cmd.core.delete(&format!("/v1/chat/sessions/{}/share", enc(id)))?;
    if cmd.json {
        return Ok(cmd.print(&result));
    }
    let revoked = a(&result, "revoked");
    if revoked.is_empty() {
        cmd.out(format!("{} {id} was not shared; only you can read it.", cmd.style.ok));
        return Ok(0);
    }
    let versions = revoked
        .iter()
        .map(|item| format!("v{}", js_string(item.get("version"))))
        .collect::<Vec<_>>()
        .join(", ");
    cmd.out(format!(
        "{} Only you can read {id} now: revoked {versions}.",
        cmd.style.ok
    ));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit_code(result: Result<Vec<Value>>) -> (i32, String) {
        match result {
            Err(Error::Cli(error)) => (error.exit_code, error.message),
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    #[test]
    fn images_are_read_as_base64_and_checked_before_anything_is_sent() {
        let dir = std::env::temp_dir().join(format!("nexus-cli-images-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("shot.png"), b"\x89PNG\r\n\x1a\n").expect("write");
        std::fs::write(dir.join("big.png"), vec![0u8; MAX_IMAGE_BYTES + 1]).expect("write");

        assert!(read_images(None, &dir).expect("none").is_empty());
        let images = read_images(Some("shot.png, shot.png"), &dir).expect("two");
        assert_eq!(images.len(), 2);
        assert_eq!(images[0], json!({ "data": "iVBORw0KGgo=", "name": "shot.png" }));

        assert_eq!(
            exit_code(read_images(Some(" , "), &dir)),
            (2, "--image needs a file.".into())
        );
        assert_eq!(
            exit_code(read_images(Some("nope.png"), &dir)),
            (2, "Cannot read nope.png.".into())
        );
        assert_eq!(
            exit_code(read_images(Some("big.png"), &dir)),
            (2, "big.png is 5.0 MB; an image may be at most 5 MB.".into())
        );
        let nine = ["shot.png"; MAX_IMAGES + 1].join(",");
        assert_eq!(
            exit_code(read_images(Some(&nine), &dir)),
            (2, "At most 8 images go with one message.".into())
        );
        // Five of 4.5 MB: each under the image limit, together over the message's 20 MB.
        let part = vec![0u8; MAX_IMAGE_BYTES * 9 / 10];
        for n in 0..5 {
            std::fs::write(dir.join(format!("part{n}.png")), &part).expect("write");
        }
        assert_eq!(
            exit_code(read_images(
                Some("part0.png,part1.png,part2.png,part3.png,part4.png"),
                &dir
            )),
            (2, "The images total 22.5 MB; one message takes at most 20 MB.".into())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_session_refusal_names_the_session_and_a_budget_one_says_who_can_raise_it() {
        let shown = |code: &str, nexus: Value| {
            let body = json!({ "error": { "message": "refused", "code": code, "nexus": nexus } });
            let error = Error::Core(Box::new(nexus_client::error::core_error(402, &body)));
            crate::error::to_cli_error(&session_refusal(error, "chs_1"), "http://core.test")
                .hint
                .unwrap_or_default()
        };
        let kept = "The refusal is kept in session chs_1; `nexus chat sessions show chs_1`. Try another --model, or `nexus models`.";
        let raise = "Check `nexus spend`. Each organization has a budget; its owner or an admin can raise it.";
        let told = "Your organization's owners and admins were notified.";

        let notified = shown(
            "budget_exceeded",
            json!({ "session": "chs_1", "level": "organization", "notified": true }),
        );
        assert_eq!(notified, format!("{kept} {raise} {told}"));
        let untold = shown(
            "budget_exceeded",
            json!({ "session": "chs_1", "level": "organization" }),
        );
        assert_eq!(untold, format!("{kept} {raise}"));
        assert_eq!(
            shown("upstream_forbidden", json!({ "session": "chs_1", "notified": true })),
            kept
        );
    }
}
