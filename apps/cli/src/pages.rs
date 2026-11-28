//! `nexus pages` (`pages.ts`): standalone HTML or Markdown pages in versions that never change. `share`
//! makes an artifact of kind `page` for the same audiences as a chat; readers open it in a sandboxed frame. `--base` names the version a change was built on: Gateway
//! refuses it (409 `page_version_conflict`) when someone wrote a newer one since. The title defaults to the file's
//! `<title>` or first `# ` heading, else its name. `open` opens a page or a shared page in Nexus UI's viewer.
//! `frame` prints the signed sandbox URL that renders it, so an agent with no browser sign-in can look at it.
//! `check` lists what a design has off its design system's tokens, below WCAG AA contrast, and out of line in its layout.
//! `export` writes the page as a file, as Nexus UI's Export menu does, and names each board that did not
//! render.

use serde_json::{Value, json};

use crate::artifacts::audience_text;
use crate::catalog::PAGE_COMMANDS;
use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{CliError, Error, Result, usage};
use crate::json::{a, js_string, obj, s};
use crate::style::{Style, ago, col, kv, list, pad_end, pairs, right, rows_of};
use nexus_client::blocking::Download;

pub fn pages_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus pages <command>", style.bold("Usage")),
        String::new(),
        "Pages are standalone HTML or Markdown files, kept in versions that never change. Share one to let others open it."
            .to_string(),
        String::new(),
        style.bold("Commands"),
    ];
    lines.extend(
        PAGE_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 48)))),
    );
    lines
}

/// The command names of `nexus pages help --json`, once each.
pub fn page_command_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for (name, _) in PAGE_COMMANDS {
        let first = name.split(' ').next().unwrap_or("");
        if !names.contains(&first) {
            names.push(first);
        }
    }
    names
}

/// `md` for `.md` and `.markdown` files, `jsx` for `.jsx` and `.tsx` (a React component), `sheet` for `.sheet.json`
/// (a spreadsheet), `board` for `.board.json` (a whiteboard, or the layout of one), else `html`.
pub fn format_of(file: &str) -> &'static str {
    let lower = file.to_ascii_lowercase();
    if lower.ends_with(".sheet.json") {
        "sheet"
    } else if lower.ends_with(".md") || lower.ends_with(".markdown") {
        "md"
    } else if lower.ends_with(".board.json") {
        "board"
    } else if lower.ends_with(".jsx") || lower.ends_with(".tsx") {
        "jsx"
    } else {
        "html"
    }
}

/// A board's own title: its `title`, or its layout's (`boardTitle` in `pages.ts`).
fn board_title(content: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    value
        .get("title")
        .or_else(|| value.get("layout").and_then(|layout| layout.get("title")))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// An HTML `<title>`'s text, as `pages.ts` finds it (`/<title[^>]*>([^<]*)<\/title>/i`).
fn html_title(content: &str) -> Option<&str> {
    let lower = content.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<title") {
        let start = from + at;
        let close = lower[start..].find('>')?;
        let text_start = start + close + 1;
        let text_end = lower[text_start..]
            .find('<')
            .map_or(lower.len(), |end| text_start + end);
        if lower[text_end..].starts_with("</title>") {
            return Some(&content[text_start..text_end]);
        }
        from = start + 1;
    }
    None
}

/// A Markdown `# ` heading's text, as `pages.ts` finds it (`/^#[ \t]+(.+)$/m`).
fn markdown_title(content: &str) -> Option<&str> {
    content.split(['\n', '\r']).find_map(|line| {
        let rest = line.strip_prefix('#')?;
        (rest.starts_with([' ', '\t']) && rest.chars().count() >= 2).then_some(rest)
    })
}

/// The page's own title: an HTML `<title>`, a Markdown `# ` heading, else (and for a React component) the file name
/// without its extension.
pub fn title_of(file: &str, content: &str, format: &str) -> String {
    let board = (format == "board").then(|| board_title(content)).flatten();
    let found = match format {
        "md" => markdown_title(content),
        "jsx" | "sheet" => None,
        "board" => board.as_deref(),
        _ => html_title(content),
    };
    let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let lower = base.to_ascii_lowercase();
    // `hiring.sheet.json` is the sheet `hiring`.
    let base = match lower.strip_suffix(".sheet.json") {
        Some(stem) if !stem.is_empty() => &base[..stem.len()],
        _ if lower.ends_with(".board.json") => &base[..base.len() - ".json".len()],
        _ => base,
    };
    let name = match base.rfind('.') {
        Some(at) if at + 1 < base.len() => &base[..at],
        _ => base,
    };
    let title = found.unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
    let title = if title.is_empty() { name.to_string() } else { title };
    title.chars().take(120).collect()
}

fn version_number(value: Option<&str>, what: &str) -> Result<i64> {
    match value.and_then(|value| value.parse::<i64>().ok()) {
        Some(number) if number >= 1 => Ok(number),
        _ => usage(format!("{what} must be a version number (1 or more)."), None),
    }
}

fn read(cmd: &Cmd, file: &str) -> Result<String> {
    cmd.read_file(file)
        .or_else(|error| usage(format!("Could not read {file}: {error}"), None))
}

fn short(title: &str) -> String {
    if title.chars().count() > 40 {
        format!("{}…", title.chars().take(39).collect::<String>())
    } else {
        title.to_string()
    }
}

fn written(result: &Value, verb: &str, cmd: &Cmd) -> Result<i32> {
    if cmd.json {
        return Ok(cmd.print(result));
    }
    let page = &result["page"];
    let version = &result["version"];
    let id = js_string(page.get("id"));
    cmd.out(format!(
        "{} {verb} {id}, version {} ({} bytes): {}",
        cmd.style.ok,
        js_string(version.get("version")),
        js_string(version.get("bytes")),
        js_string(page.get("title")),
    ));
    // The viewer follows the latest version, so the link stays right after later updates. `/a/<page id>` opens for
    // the owner and for each share's readers, so the link still works once the page is shared.
    let link = crate::artifacts::viewer_link(cmd.config, &id);
    cmd.out(format!("{} View it: {}", cmd.style.arrow, cmd.style.cyan(&link)));
    Ok(0)
}

/// `get`'s 404 names the Gateway and organization it looked in (`pageNotFound` in `pages.ts`). Nexus for desktop and
/// this CLI can sign in to different ones, so "no page" alone reads as if the page were gone. Only the page itself: a
/// missing version keeps Gateway's own words.
fn page_not_found(id: &str, config: &nexus_client::config::Config, error: Error) -> Error {
    match &error {
        Error::Core(core) if core.status == 404 && core.code.as_deref() == Some("page_not_found") => {
            let org = config
                .extra
                .get("org")
                .and_then(Value::as_str)
                .filter(|org| !org.is_empty());
            let org = org.map(|org| format!(" (organization {org})")).unwrap_or_default();
            Error::Cli(
                CliError::hint(
                    format!("No page {id} on Gateway {}{org}.", config.url),
                    "This CLI reads the Gateway it signed in to. If Nexus for desktop shows the page, it uses another \
                     Gateway or organization: run `NEXUS_GATEWAY=<its Gateway> nexus pages get …` to read it there.",
                )
                .with_code("page_not_found"),
            )
        }
        _ => error,
    }
}

pub fn pages_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    let first = args.get(1).filter(|id| !id.is_empty()).map(String::as_str);
    let second = args.get(2).map(String::as_str);
    let need = |what: &str| -> Result<&str> {
        match first {
            Some(first) => Ok(first),
            None => usage(
                format!("Usage: nexus pages {action} {what}"),
                Some("`nexus pages` lists your pages."),
            ),
        }
    };
    let path = |page: &str, suffix: &str| format!("/v1/pages/{}{suffix}", enc(page));
    let base = |body: &mut Value| -> Result<()> {
        if cmd.flag("base").is_some() || cmd.has("base") {
            body["baseVersion"] = json!(version_number(cmd.flag("base"), "--base")?);
        }
        Ok(())
    };

    if action == "help" || (cmd.has("help") && args.is_empty()) {
        if cmd.json {
            return Ok(cmd.print(&json!({ "usage": "nexus pages <command>", "commands": page_command_names() })));
        }
        cmd.lines(pages_help(style));
        return Ok(0);
    }

    match action {
        "publish" => {
            let file = need("<file>")?;
            let content = read(cmd, file)?;
            let format = match cmd.flag("format") {
                None => format_of(file),
                Some("html") => "html",
                Some("md") => "md",
                Some("jsx") => "jsx",
                Some("sheet") => "sheet",
                Some("board") => "board",
                Some(_) => return usage("--format must be html, md, jsx, sheet or board.", None),
            };
            let title = cmd
                .flag_set_str("title")
                .unwrap_or_else(|| title_of(file, &content, format));
            let result = cmd.core.post(
                "/v1/pages",
                json!({ "title": title, "format": format, "content": content }),
            )?;
            written(&result, "Published", cmd)
        }
        "list" => {
            let page = cmd.core.get("/v1/pages")?;
            if cmd.json {
                return Ok(cmd.print(&page));
            }
            let pages = a(&page, "pages");
            if pages.is_empty() {
                cmd.out(format!(
                    "No pages yet. Publish one: {}",
                    style.cyan("nexus pages publish report.html")
                ));
                return Ok(0);
            }
            let rows: Vec<Vec<String>> = pages
                .iter()
                .map(|item| {
                    vec![
                        js_string(item.get("id")),
                        short(&js_string(item.get("title"))),
                        js_string(item.get("format")),
                        format!("v{}", js_string(item.get("latestVersion"))),
                        ago(s(item, "updatedAt").unwrap_or("")),
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[
                    col("Page"),
                    col("Title"),
                    col("Format"),
                    right("Version"),
                    col("Updated"),
                ],
                rows_of(rows),
                None,
            ));
            if page.get("nextCursor").is_some_and(|cursor| !cursor.is_null()) {
                cmd.out(style.dim("More: the API pages with ?cursor=… (GET /v1/pages)."));
            }
            Ok(0)
        }
        "get" => {
            let id = need("<id>")?;
            let version = match second {
                None => "latest".to_string(),
                Some(value) => version_number(Some(value), "The version")?.to_string(),
            };
            if cmd.has("out") {
                return usage("--out needs a file name.", None);
            }
            let out = cmd.args.flag_set("out");
            if cmd.has("source") || out.is_some() {
                let source = cmd
                    .core
                    .get_text(&path(id, &format!("/versions/{version}/source")))
                    .map_err(|error| page_not_found(id, cmd.config, error))?;
                let version_json = match version.parse::<i64>() {
                    Ok(number) => json!(number),
                    Err(_) => json!(version),
                };
                if let Some(out) = out {
                    std::fs::write(cmd.ctx.cwd.join(out), &source)
                        .map_err(|error| Error::Cli(CliError::new(format!("Could not write {out}: {error}"))))?;
                    if cmd.json {
                        return Ok(cmd
                            .print(&json!({ "id": id, "version": version_json, "path": out, "bytes": source.len() })));
                    }
                    cmd.out(format!("{} Wrote {out} ({} bytes).", style.ok, source.len()));
                    return Ok(0);
                }
                if cmd.json {
                    return Ok(cmd.print(&json!({ "id": id, "version": version_json, "source": source })));
                }
                let text = if source.ends_with('\n') {
                    source.clone()
                } else {
                    format!("{source}\n")
                };
                if !cmd.ctx.io.write(&text) {
                    cmd.out(source.strip_suffix('\n').unwrap_or(&source));
                }
                return Ok(0);
            }
            let one = cmd
                .core
                .get(&path(id, &format!("/versions/{version}")))
                .map_err(|error| page_not_found(id, cmd.config, error))?;
            if cmd.json {
                return Ok(cmd.print(&one));
            }
            let restored = one
                .get("restoredFrom")
                .filter(|from| !from.is_null())
                .map(|from| format!(" (from v{})", js_string(Some(from))))
                .unwrap_or_default();
            cmd.lines(pairs(
                style,
                &[
                    ("Page".into(), js_string(one.get("pageId"))),
                    ("Title".into(), js_string(one.get("title"))),
                    ("Version".into(), format!("v{}", js_string(one.get("version")))),
                    ("Size".into(), format!("{} bytes", js_string(one.get("bytes")))),
                    (
                        "Written".into(),
                        format!(
                            "{} via {}{restored}",
                            ago(s(&one, "createdAt").unwrap_or("")),
                            js_string(one.get("createdVia"))
                        ),
                    ),
                    ("SHA-256".into(), js_string(one.get("contentSha256"))),
                ],
            ));
            Ok(0)
        }
        "update" => {
            let id = need("<id> <file>")?;
            let Some(file) = second else {
                return usage("Usage: nexus pages update <id> <file>", None);
            };
            let content = read(cmd, file)?;
            let mut body = json!({ "content": content });
            if let Some(title) = cmd.flag_set_str("title") {
                body["title"] = json!(title);
            }
            base(&mut body)?;
            let result = cmd.core.post(&path(id, "/versions"), body)?;
            written(&result, "Wrote", cmd)
        }
        "versions" => {
            let detail = cmd.core.get(&path(need("<id>")?, ""))?;
            if cmd.json {
                return Ok(cmd.print(&detail));
            }
            cmd.out(format!(
                "{} {}",
                style.bold(&js_string(detail.get("title"))),
                style.dim(&format!(
                    "{} · {}",
                    js_string(detail.get("id")),
                    js_string(detail.get("format"))
                ))
            ));
            let rows: Vec<Vec<String>> = a(&detail, "versions")
                .iter()
                .map(|item| {
                    let via = match item.get("restoredFrom").filter(|from| !from.is_null()) {
                        Some(from) => format!("restore of v{}", js_string(Some(from))),
                        None => js_string(item.get("createdVia")),
                    };
                    vec![
                        format!("v{}", js_string(item.get("version"))),
                        short(&js_string(item.get("title"))),
                        js_string(item.get("bytes")),
                        via,
                        ago(s(item, "createdAt").unwrap_or("")),
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[
                    right("Version"),
                    col("Title"),
                    right("Bytes"),
                    col("Via"),
                    col("Written"),
                ],
                rows_of(rows),
                None,
            ));
            Ok(0)
        }
        "restore" => {
            let id = need("<id> <version>")?;
            let version = version_number(second, "The version to restore")?;
            let mut body = json!({ "version": version });
            base(&mut body)?;
            let result = cmd.core.post(&path(id, "/restore"), body)?;
            written(&result, &format!("Restored v{version} of"), cmd)
        }
        "delete" => {
            let id = need("<id>")?;
            if !cmd.has("yes") {
                return usage(
                    format!("Deleting {id} removes every version for good."),
                    Some(&format!("nexus pages delete {id} --yes")),
                );
            }
            let deleted = cmd.core.delete(&path(id, ""))?;
            if cmd.json {
                return Ok(cmd.print(&deleted));
            }
            let versions = deleted.get("versions").and_then(Value::as_i64).unwrap_or(0);
            cmd.out(format!(
                "{} Deleted {} and its {versions} {}.",
                style.ok,
                js_string(deleted.get("id")),
                if versions == 1 { "version" } else { "versions" }
            ));
            Ok(0)
        }
        "share" => share(need("<id>")?, cmd),
        "open" => {
            let id = need("<id|art_id> [<version>]")?;
            let version = second
                .map(|value| version_number(Some(value), "The version"))
                .transpose()?;
            let link = crate::artifacts::ui_link(cmd.config, &viewer_path(id, version)?);
            if cmd.json {
                return Ok(cmd.print(&json!({ "id": id, "url": link })));
            }
            if !cmd.has("no-browser") && cmd.ctx.io.open_url(&link) {
                cmd.out(format!("{} Opened {}", style.ok, style.cyan(&link)));
            } else {
                cmd.out(format!("{} Open it in a browser: {}", style.arrow, style.cyan(&link)));
            }
            Ok(0)
        }
        "frame" => {
            let id = need("<id|art_id> [<version>]")?;
            let version = second
                .map(|value| version_number(Some(value), "The version"))
                .transpose()?;
            let frame = cmd.core.get(&frame_path(id, version)?)?;
            if cmd.json {
                return Ok(cmd.print(&frame));
            }
            // The URL alone, so `$(nexus pages frame …)` is the URL.
            cmd.out(js_string(frame.get("url")));
            Ok(0)
        }
        "check" => {
            let id = need("<id> [<version>]")?;
            let body = match second {
                Some(value) => json!({ "version": version_number(Some(value), "The version")? }),
                None => json!({}),
            };
            let check = cmd.core.post(&path(id, "/check"), body)?;
            if cmd.json {
                return Ok(cmd.print(&check));
            }
            print_check(&check, cmd);
            Ok(0)
        }
        "export" => {
            let id = need("<id> [<version>] --format png|svg|pdf|pptx|html|zip")?;
            let version = second
                .map(|value| version_number(Some(value), "The version"))
                .transpose()?;
            export(id, version, cmd)
        }
        _ => usage(
            format!("Unknown command `nexus pages {action}`."),
            Some("Use publish, list, get, update, versions, restore, delete, share, open, frame, check or export."),
        ),
    }
}

/// `nexus pages check`'s answer (`printCheck` in pages.ts): the counts, then each finding with its board and layer.
fn print_check(check: &Value, cmd: &Cmd) {
    let style = cmd.style;
    let count = |name: &str| check["counts"][name].as_i64().unwrap_or(0);
    let (color, font_size, spacing, contrast) =
        (count("color"), count("fontSize"), count("spacing"), count("contrast"));
    let (overflow, clipped, outside, misaligned) = (
        count("overflow"),
        count("clippedText"),
        count("outsideBoard"),
        count("misalignedRows"),
    );
    let layout = overflow + clipped + outside + misaligned;
    let total = color + font_size + spacing + contrast + layout;
    let system = js_string(check["designSystem"].get("name"));
    let against = if check.get("linked").and_then(Value::as_bool) == Some(true) {
        system
    } else {
        format!("{system}, the default: the page links none")
    };
    cmd.out(format!(
        "{} {} v{} against {}: {} elements checked.",
        if total > 0 { &style.warn } else { &style.ok },
        js_string(check.get("pageId")),
        js_string(check.get("version")),
        style.bold(&against),
        js_string(check.get("elements")),
    ));
    if total == 0 {
        cmd.out(format!(
            "{} Every colour, font size and spacing is on the tokens, every text passes WCAG AA contrast, and the layout has no overflow, clipping or misalignment.",
            style.arrow
        ));
        return;
    }
    cmd.out(format!(
        "  {color} colours, {font_size} font sizes and {spacing} spacings off the tokens; {contrast} below AA contrast."
    ));
    if layout > 0 {
        cmd.out(format!(
            "  Layout: {overflow} overflowing, {clipped} clipped text, {outside} outside their board, {misaligned} misaligned rows."
        ));
    }
    let findings = a(check, "findings");
    let rows: Vec<Vec<String>> = findings
        .iter()
        .map(|finding| {
            let place: Vec<String> = ["board", "layer"]
                .iter()
                .filter_map(|key| finding.get(*key).and_then(Value::as_str))
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect();
            vec![
                js_string(finding.get("kind")),
                if place.is_empty() {
                    js_string(finding.get("selector"))
                } else {
                    place.join(" › ")
                },
                js_string(finding.get("property")),
                js_string(finding.get("value")),
                finding
                    .get("expected")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ]
        })
        .collect();
    cmd.lines(list(
        style,
        &[col("Kind"), col("Where"), col("Property"), col("Value"), col("Nearest")],
        rows_of(rows),
        None,
    ));
    if check.get("truncated").and_then(Value::as_bool) == Some(true) {
        cmd.out(style.dim(&format!(
            "The first {} of {total}; --json gives each with its selector and box.",
            findings.len()
        )));
    }
}

/// The files `nexus pages export` writes.
pub const EXPORT_FORMATS: [&str; 6] = ["png", "svg", "pdf", "pptx", "html", "zip"];

/// Gateway's export of one page: `/v1/pages/<id>/export?format&board&scale&version`, as `exportPath` builds it.
pub fn export_path(id: &str, format: &str, board: Option<&str>, scale: Option<&str>, version: Option<i64>) -> String {
    let query = crate::client::search(&[
        ("format", Some(format.to_string())),
        ("board", board.map(String::from)),
        ("scale", scale.map(String::from)),
        ("version", version.map(|version| version.to_string())),
    ]);
    format!("/v1/pages/{}/export{query}", enc(id))
}

/// The file `export` writes without `--out`: `<id>[-<board>].<format>`.
pub fn export_file(id: &str, format: &str, board: Option<&str>) -> String {
    let lower = board.unwrap_or("").to_ascii_lowercase();
    let part: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect();
    if part.is_empty() {
        format!("{id}.{format}")
    } else {
        format!("{id}-{}.{format}", part.join("-"))
    }
}

/// The boards an export did not render, from `x-nexus-export-failed`.
pub fn failed_boards(header: Option<&str>) -> Vec<(String, String)> {
    let Some(text) = header.and_then(nexus_client::base::decode_uri_component) else {
        return Vec::new();
    };
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| Some((s(entry, "board")?.to_string(), s(entry, "error")?.to_string())))
        .collect()
}

fn export(id: &str, version: Option<i64>, cmd: &Cmd) -> Result<i32> {
    let format = cmd.flag("format").unwrap_or("");
    if !EXPORT_FORMATS.contains(&format) {
        return usage(
            "--format must be png, svg, pdf, pptx, html or zip.",
            Some("nexus pages export pag_… --format pdf"),
        );
    }
    if id.starts_with("art_") {
        return usage(
            "Export a page by its own id (pag_…).",
            Some("`nexus artifacts get <art_id> --json` shows a shared page's pageId."),
        );
    }
    if cmd.has("board") || cmd.flag("board") == Some("") {
        return usage("--board needs a board's id, name or number.", None);
    }
    let scale = cmd.flag("scale");
    if cmd.has("scale") || scale.is_some_and(|scale| scale != "1" && scale != "2") {
        return usage("--scale must be 1 or 2.", None);
    }
    if cmd.has("out") {
        return usage("--out needs a file name.", None);
    }
    let board = cmd.flag("board");
    let file = match cmd.core.download(&export_path(id, format, board, scale, version))? {
        Download::File(file) => file,
        Download::Cut(_) | Download::TimedOut => {
            return Err(CliError::new(
                "The file is cut: Nexus Gateway did not send all of it. Nothing was written; run the command again.",
            )
            .into());
        }
    };
    let target = cmd
        .flag_set_str("out")
        .unwrap_or_else(|| export_file(id, format, board));
    std::fs::write(cmd.ctx.cwd.join(&target), &file.body)
        .map_err(|error| Error::Cli(CliError::new(format!("Could not write {target}: {error}"))))?;
    let failed = failed_boards(file.failed.as_deref());
    if cmd.json {
        let failed: Vec<Value> = failed
            .iter()
            .map(|(board, error)| json!({ "board": board, "error": error }))
            .collect();
        return Ok(cmd.print(
            &json!({ "id": id, "format": format, "path": target, "bytes": file.body.len(), "failed": failed }),
        ));
    }
    let style = cmd.style;
    cmd.out(format!("{} Wrote {target} ({} bytes).", style.ok, file.body.len()));
    for (board, error) in failed {
        cmd.out(format!("{} {board} did not render: {error}", style.warn));
    }
    Ok(0)
}

/// Where Nexus UI shows a page: `/pages/<id>` (`?v=N` for a version), or a shared page's `/artifacts/<art_id>`. A
/// shared page follows the version its share pins, so it takes no version.
pub fn viewer_path(id: &str, version: Option<i64>) -> Result<String> {
    if id.starts_with("art_") {
        if version.is_some() {
            return usage(
                "A shared page opens at the version its share pins; open the page by its own id for another one.",
                None,
            );
        }
        return Ok(format!("/artifacts/{}", enc(id)));
    }
    Ok(match version {
        Some(version) => format!("/pages/{}?v={version}", enc(id)),
        None => format!("/pages/{}", enc(id)),
    })
}

/// Where Gateway mints a page's signed frame URL: the owner's `/v1/pages/<id>/versions/<N|latest>/frame`, or a shared
/// page's `/v1/artifacts/<art_id>/frame`, which takes no version for the same reason as [`viewer_path`].
pub fn frame_path(id: &str, version: Option<i64>) -> Result<String> {
    if id.starts_with("art_") {
        if version.is_some() {
            return usage(
                "A shared page renders the version its share pins; frame the page by its own id for another one.",
                None,
            );
        }
        return Ok(format!("/v1/artifacts/{}/frame", enc(id)));
    }
    let version = version.map_or_else(|| "latest".to_string(), |version| version.to_string());
    Ok(format!("/v1/pages/{}/versions/{version}/frame", enc(id)))
}

/// `nexus pages share`: exactly one audience, as `nexus chat share` takes it; teams and people comma-separated.
fn share(id: &str, cmd: &Cmd) -> Result<i32> {
    let usage_text = "Usage: nexus pages share <id> [--org | --team T | --people a@x,b@y | --to a@x,b@y | --public [--expires 30d]] [--pin N] [--json]";
    let team = cmd.flag("team").is_some() || cmd.has("team");
    let people = cmd.flag("people").is_some() || cmd.has("people");
    let to = cmd.flag("to").is_some() || cmd.has("to");
    let public = cmd.has("public");
    let chosen = [cmd.has("org"), team, people, to, public]
        .iter()
        .filter(|chosen| **chosen)
        .count();
    if chosen != 1 {
        return usage(
            "Choose who may open it: --org, --team T, --people a@x,b@y, --to a@x,b@y or --public (one of them).",
            Some(usage_text),
        );
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
    let pinned = if cmd.flag("pin").is_some() || cmd.has("pin") {
        Some(json!(version_number(cmd.flag("pin"), "--pin")?))
    } else {
        None
    };
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
    let body = obj(vec![
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
        ("version", pinned),
    ]);
    let artifact = cmd.core.post(&format!("/v1/pages/{}/share", enc(id)), body)?;
    if cmd.json {
        return Ok(cmd.print(&artifact));
    }
    let style = cmd.style;
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
        "{} Shared {} {whom}.",
        style.ok,
        style.bold(&js_string(artifact.get("title")))
    ));
    let present = |key: &str| artifact.get(key).filter(|value| !value.is_null());
    let aid = js_string(artifact.get("id"));
    let mut rows = vec![
        kv("Artifact", aid.clone()),
        kv("Open", js_string(present("publicUrl").or_else(|| artifact.get("url")))),
        kv(
            "Version",
            match present("pageVersion") {
                Some(version) => format!("v{}, pinned", js_string(Some(version))),
                None => "the latest, as you write new ones".into(),
            },
        ),
    ];
    if kind == "public" {
        rows.push(kv(
            "Expires",
            present("expiresAt").map_or_else(|| "never".into(), |at| js_string(Some(at))),
        ));
    }
    if kind == "external" {
        rows.push(kv(
            "Sign-in",
            "readers open it signed in with that email; anyone else can ask you for access",
        ));
    }
    rows.push(kv("Revoke", style.cyan(&format!("nexus artifacts revoke {aid}"))));
    cmd.lines(pairs(style, &rows));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_title_and_format_from_the_file() {
        assert_eq!(format_of("a/NOTES.Markdown"), "md");
        assert_eq!(format_of("index.htm"), "html");
        assert_eq!(format_of("dash/App.TSX"), "jsx");
        assert_eq!(format_of("chart.jsx"), "jsx");
        assert_eq!(format_of("plan/Q1.board.json"), "board");
        assert_eq!(format_of("data.json"), "html");
        assert_eq!(
            title_of(
                "plan/q1.board.json",
                r#"{"layout":"timeline","title":"Q1 launch"}"#,
                "board"
            ),
            "Q1 launch"
        );
        assert_eq!(
            title_of(
                "plan/q1.board.json",
                r#"{"kind":"board","version":1,"shapes":{}}"#,
                "board"
            ),
            "q1"
        );
        assert_eq!(format_of("hiring.Sheet.JSON"), "sheet");
        assert_eq!(title_of("plans/hiring.sheet.json", "{}", "sheet"), "hiring");
        assert_eq!(
            title_of("dash/Revenue.jsx", "<title>not this</title>", "jsx"),
            "Revenue"
        );
        assert_eq!(
            title_of("x.html", "<TITLE> Q3\n revenue </TITLE>", "html"),
            "Q3 revenue"
        );
        assert_eq!(title_of("x.html", "<title>a<b></title>", "html"), "x");
        assert_eq!(title_of("dir/notes.md", "no heading", "md"), "notes");
        assert_eq!(title_of("x.md", "#hashtag\n#  Real title\n", "md"), "Real title");
        assert_eq!(title_of("C:\\r\\q3.html", "", "html"), "q3");
        assert_eq!(
            title_of("x.html", &format!("<title>{}</title>", "é".repeat(130)), "html")
                .chars()
                .count(),
            120
        );
    }

    #[test]
    fn opens_a_page_or_a_shared_page_in_the_viewer() {
        assert_eq!(viewer_path("pag_1", None).expect("a viewer path"), "/pages/pag_1");
        assert_eq!(
            viewer_path("pag_1", Some(3)).expect("a viewer path"),
            "/pages/pag_1?v=3"
        );
        assert_eq!(viewer_path("art_9", None).expect("a viewer path"), "/artifacts/art_9");
        assert!(viewer_path("art_9", Some(2)).is_err());
    }

    #[test]
    fn exports_to_the_path_and_file_the_typescript_cli_uses() {
        assert_eq!(
            export_path("pag_1", "png", Some("M1 Phone"), Some("2"), Some(3)),
            "/v1/pages/pag_1/export?format=png&board=M1+Phone&scale=2&version=3"
        );
        assert_eq!(
            export_path("pag_1", "zip", None, None, None),
            "/v1/pages/pag_1/export?format=zip"
        );
        assert_eq!(export_file("pag_1", "png", Some("M1 · Phone")), "pag_1-m1-phone.png");
        assert_eq!(export_file("pag_1", "pdf", None), "pag_1.pdf");
        assert_eq!(
            failed_boards(Some("%5B%7B%22board%22%3A%22Phone%22%2C%22error%22%3A%22boom%22%7D%5D")),
            [("Phone".to_string(), "boom".to_string())]
        );
        assert!(failed_boards(Some("%ZZ")).is_empty());
        assert!(failed_boards(None).is_empty());
    }

    #[test]
    fn frames_the_latest_version_a_named_one_or_a_shared_page() {
        assert_eq!(
            frame_path("pag_1", None).expect("a frame path"),
            "/v1/pages/pag_1/versions/latest/frame"
        );
        assert_eq!(
            frame_path("pag_1", Some(3)).expect("a frame path"),
            "/v1/pages/pag_1/versions/3/frame"
        );
        assert_eq!(
            frame_path("art_9", None).expect("a frame path"),
            "/v1/artifacts/art_9/frame"
        );
        assert!(frame_path("art_9", Some(2)).is_err());
    }
}
