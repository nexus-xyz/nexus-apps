//! `nexus skills promote | promotions | approve | refuse | withdraw` (`skill-promotions.ts`): the promotion of
//! a version of a person's skill to the organization or a team. The person asks, an approver who is not that person
//! approves or refuses, and an approval adds the version to the library (`skill_library.rs` lists it). The API is
//! `/v1/skills/{id}/versions/{version}/promotions` and `/v1/skill-promotions`.
//!
//! The text of a request belongs to another person, so every field of it goes through `safe_text` before it reaches the
//! terminal, and `--json` goes through `safe_json`. The diff is from the last approved version of the same library skill
//! (`previous` of `GET /v1/skill-promotions/{id}`). A saved API key may not decide: Gateway says so
//! (`person_session_required`) and the fix line says to run `nexus auth login`.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::cli::Cmd;
use crate::client::{enc, search};
use crate::error::{Error, Result, usage};
use crate::json::{a, pretty, s};
use crate::safe_text::{safe_json, safe_text};
use crate::skill_errors::refused;
use crate::skills::{find, one_line, version_number};
use crate::style::{col, flex, list, pairs, quote, right, rows_of};
use crate::teams::js_integer;
use crate::text_diff::unified_diff;

/// The states of a promotion request, as `GET /v1/skill-promotions?state=` reads them.
const STATES: [&str; 4] = ["held", "approved", "refused", "withdrawn"];

/// The longest note or reason, in characters (Gateway's limit).
const MAX_TEXT_CHARS: usize = 500;

/// Where a promotion goes, or which part of the library to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Org,
    Team(String),
}

/// `org` or `team/<slug>`, the way `nexus budget set` names a team. `org` and `team/` may be in any case; the slug stays
/// as typed. Anything else is a usage error.
pub fn parse_target(text: &str) -> Result<Target> {
    if text.eq_ignore_ascii_case("org") {
        return Ok(Target::Org);
    }
    let slug = match text.get(..5) {
        Some(prefix) if prefix.eq_ignore_ascii_case("team/") => text.get(5..).filter(|slug| !slug.is_empty()),
        _ => None,
    };
    match slug {
        Some(slug) if !slug.chars().any(|c| c.is_whitespace() || c.is_control() || c == '/') => {
            Ok(Target::Team(slug.to_string()))
        }
        _ => usage(
            format!("{} is not a target.", quote(&safe_text(text))),
            Some("Name it as org or team/<slug>."),
        ),
    }
}

/// A flag that needs a value: `None` when it is not given; a usage error when it is given with none.
pub(crate) fn value_flag<'c>(cmd: &'c Cmd, name: &str, needs: &str) -> Result<Option<&'c str>> {
    if cmd.has(name) || cmd.args.flag(name) == Some("") {
        return usage(format!("--{name} needs {needs}."), None);
    }
    Ok(cmd.args.flag_set(name))
}

/// A `--note` or `--reason`: trimmed, with 1 to 500 characters when it is given.
fn text_flag(cmd: &Cmd, name: &str) -> Result<Option<String>> {
    let Some(raw) = value_flag(cmd, name, "text")? else {
        return Ok(None);
    };
    let text = raw.trim();
    if text.is_empty() {
        return usage(format!("--{name} needs text."), None);
    }
    let chars = text.chars().count();
    if chars > MAX_TEXT_CHARS {
        return usage(
            format!("--{name} is at most {MAX_TEXT_CHARS} characters; this one is {chars}."),
            None,
        );
    }
    Ok(Some(text.to_string()))
}

/// The argument after the verb (`nexus skills approve <id>`), or the usage line with a hint where to find one.
fn id_arg<'a>(args: &'a [String], usage_line: &str, hint: &str) -> Result<&'a str> {
    match args.get(1).filter(|id| !id.is_empty()) {
        Some(id) => Ok(id),
        None => usage(format!("Usage: {usage_line}"), Some(hint)),
    }
}

const FIND_SKILL: &str = "`nexus skills` lists your skills.";
const FIND_REQUEST: &str = "`nexus skills promotions` lists the requests.";

/// Text on one line, safe to print: a CRLF is a line end, each other hazard is shown as its code, then runs of white
/// space become one space.
pub(crate) fn safe_line(text: &str) -> String {
    one_line(&safe_text(&text.replace("\r\n", "\n")))
}

/// How the lines of a text end: `LF`, `CRLF` or `mixed`. A text with no line end counts as `LF`.
fn line_ends(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    if crlf == 0 {
        "LF"
    } else if crlf == text.matches('\n').count() {
        "CRLF"
    } else {
        "mixed"
    }
}

/// The text of a field, safe to print; empty when it is missing.
fn text(value: &Value, key: &str) -> String {
    safe_text(s(value, key).unwrap_or(""))
}

fn number(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_i64)
        .map(|number| number.to_string())
        .unwrap_or_default()
}

/// `--json`: the answer as Gateway sent it, with the characters a terminal could run or a reader could not see escaped.
pub(crate) fn print_json(cmd: &Cmd, value: &Value) -> i32 {
    cmd.out(safe_json(&pretty(value)));
    0
}

/// The slug of each team that an item names, by team id. A team whose slug cannot be read is shown by its id.
pub(crate) fn team_slugs(cmd: &Cmd, items: &[Value]) -> HashMap<String, String> {
    if !items.iter().any(|item| s(item, "teamId").is_some()) {
        return HashMap::new();
    }
    match cmd.core.get("/v1/teams") {
        Ok(teams) => a(&teams, "teams")
            .iter()
            .filter_map(|team| Some((s(team, "id")?.to_string(), s(team, "slug")?.to_string())))
            .collect(),
        Err(_) => HashMap::new(),
    }
}

/// Where an item is for: `organization`, or `team web` (`the organization` in a sentence).
pub(crate) fn place(item: &Value, slugs: &HashMap<String, String>, sentence: bool) -> String {
    if s(item, "scope") != Some("team") {
        return if sentence { "the organization" } else { "organization" }.to_string();
    }
    match s(item, "teamId") {
        Some(id) => format!("team {}", safe_text(slugs.get(id).map_or(id, String::as_str))),
        None => "a team".to_string(),
    }
}

// ---------------------------------------------------------------- promote

/// `nexus skills promote <name|id> [<version>] --to org|team/<slug> [--note TEXT]`.
pub fn promote(args: &[String], cmd: &Cmd) -> Result<i32> {
    const USAGE: &str = "nexus skills promote <name|id> [<version>] --to org|team/<slug> [--note TEXT]";
    let who = id_arg(args, USAGE, FIND_SKILL)?;
    let version = match args.get(2).map(String::as_str) {
        None | Some("latest") => "latest".to_string(),
        Some(value) => version_number(Some(value), "The version")?.to_string(),
    };
    let Some(to) = value_flag(cmd, "to", "a target: org or team/<slug>")? else {
        return usage(
            format!("Usage: {USAGE}"),
            Some("Name where it goes: --to org, or --to team/<slug>."),
        );
    };
    let target = parse_target(to)?;
    let note = text_flag(cmd, "note")?;
    let skill = find(cmd, who)?;

    let mut body = match &target {
        Target::Org => json!({ "scope": "org" }),
        Target::Team(slug) => json!({ "scope": "team", "team": slug }),
    };
    if let Some(note) = note {
        body["note"] = json!(note);
    }
    let promotion = cmd
        .core
        .post(
            &format!("/v1/skills/{}/versions/{version}/promotions", enc(&skill.id)),
            body,
        )
        .map_err(refused)?;
    if cmd.json {
        return Ok(print_json(cmd, &promotion));
    }
    let id = text(&promotion, "id");
    let (to_text, asked) = match &target {
        Target::Org => (
            "the organization".to_string(),
            "the organization's other owners and admins".to_string(),
        ),
        Target::Team(slug) => (
            format!("team {}", safe_text(slug)),
            format!(
                "the organization's other owners and admins, and the other leads of team {} and of the teams above it",
                safe_text(slug)
            ),
        ),
    };
    cmd.out(format!(
        "{} Requested promotion of {}, version {}, to {to_text}.",
        cmd.style.ok,
        text(&promotion, "name"),
        number(&promotion, "sourceVersion")
    ));
    cmd.lines(pairs(
        cmd.style,
        &[
            ("Request".to_string(), id.clone()),
            ("State".to_string(), text(&promotion, "state")),
            ("Asked".to_string(), asked),
            ("Next".to_string(), format!("nexus skills promotions {id}")),
        ],
    ));
    Ok(0)
}

// ---------------------------------------------------------------- promotions

/// `nexus skills promotions [--state STATE] [--to TARGET] [--mine] [--limit N] [--cursor C]`, and
/// `nexus skills promotions <id>`.
pub fn promotions(args: &[String], cmd: &Cmd) -> Result<i32> {
    if let Some(id) = args.get(1).filter(|id| !id.is_empty()) {
        return show(id, cmd);
    }
    let mut query: Vec<(&str, Option<String>)> = Vec::new();
    if let Some(state) = value_flag(cmd, "state", "a state: held, approved, refused or withdrawn")? {
        if !STATES.contains(&state) {
            return usage(
                format!("{} is not a state.", quote(&safe_text(state))),
                Some("A state is held, approved, refused or withdrawn."),
            );
        }
        query.push(("state", Some(state.to_string())));
    }
    if let Some(to) = value_flag(cmd, "to", "a target: org or team/<slug>")? {
        match parse_target(to)? {
            Target::Org => query.push(("scope", Some("org".to_string()))),
            Target::Team(slug) => {
                query.push(("scope", Some("team".to_string())));
                query.push(("team", Some(slug)));
            }
        }
    }
    if cmd.has("mine") {
        query.push(("mine", Some("true".to_string())));
    }
    if let Some(raw) = value_flag(cmd, "limit", "a number from 1 to 200")? {
        match js_integer(raw) {
            Some(limit) if (1.0..=200.0).contains(&limit) => query.push(("limit", Some(format!("{limit}")))),
            _ => return usage("--limit must be a whole number from 1 to 200.", None),
        }
    }
    if let Some(cursor) = value_flag(cmd, "cursor", "the cursor the last page printed")? {
        query.push(("cursor", Some(cursor.to_string())));
    }
    let filtered = !query.is_empty();
    let page = cmd
        .core
        .get(&format!("/v1/skill-promotions{}", search(&query)))
        .map_err(refused)?;
    if cmd.json {
        return Ok(print_json(cmd, &page));
    }
    let items = a(&page, "promotions");
    if items.is_empty() {
        if filtered {
            cmd.out("No promotion requests match.");
        } else {
            cmd.out(format!(
                "No promotion requests yet. Ask for one: {}",
                cmd.style.cyan("nexus skills promote <name> --to org")
            ));
        }
        return Ok(0);
    }
    let slugs = team_slugs(cmd, items);
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            let state = match item.get("libraryVersion").and_then(Value::as_i64) {
                Some(version) if s(item, "state") == Some("approved") => format!("approved, library v{version}"),
                _ => text(item, "state"),
            };
            vec![
                text(item, "id"),
                text(item, "name"),
                format!("v{}", number(item, "sourceVersion")),
                place(item, &slugs, false),
                state,
                cmd.style.dim(&asked_at(item)),
            ]
        })
        .collect();
    cmd.lines(list(
        cmd.style,
        &[
            col("Request"),
            col("Skill"),
            right("Version"),
            col("To"),
            col("State"),
            flex("Asked"),
        ],
        rows_of(rows),
        None,
    ));
    if let Some(next) = s(&page, "nextCursor").filter(|next| !next.is_empty()) {
        cmd.out("");
        cmd.out(
            cmd.style
                .dim(&format!("More: add --cursor {} to the same command.", safe_text(next))),
        );
    }
    Ok(0)
}

/// `2026-10-18 12:00:00`: the request time to the second.
fn asked_at(item: &Value) -> String {
    safe_text(s(item, "requestedAt").unwrap_or(""))
        .chars()
        .take(19)
        .collect::<String>()
        .replacen('T', " ", 1)
}

/// One promotion request: provenance, the diff from the last approved version, and the full text.
fn show(id: &str, cmd: &Cmd) -> Result<i32> {
    let detail = cmd
        .core
        .get(&format!("/v1/skill-promotions/{}", enc(id)))
        .map_err(refused)?;
    if cmd.json {
        return Ok(print_json(cmd, &detail));
    }
    let style = cmd.style;
    let slugs = team_slugs(cmd, std::slice::from_ref(&detail));
    cmd.out(format!(
        "{} {} {}",
        style.bold(&text(&detail, "name")),
        style.bullet,
        crate::style::meta(
            style,
            &[
                Some(format!("version {}", number(&detail, "sourceVersion"))),
                Some(text(&detail, "state")),
            ]
        )
    ));
    cmd.out(style.dim(&text(&detail, "id")));
    cmd.out("");
    let mut rows = vec![
        ("To".to_string(), place(&detail, &slugs, true)),
        (
            "Description".to_string(),
            safe_line(s(&detail, "description").unwrap_or("")),
        ),
        (
            "Asked by".to_string(),
            format!(
                "{} via {}, {}",
                text(&detail, "requestedBy"),
                text(&detail, "requestedVia"),
                text(&detail, "requestedAt")
            ),
        ),
        (
            "Written".to_string(),
            format!(
                "{} version {}, via {}, {}",
                text(&detail, "sourceSkillId"),
                number(&detail, "sourceVersion"),
                text(&detail, "sourceWrittenVia"),
                text(&detail, "sourceWrittenAt")
            ),
        ),
    ];
    if let Some(note) = s(&detail, "note") {
        rows.push(("Note".to_string(), safe_line(note)));
    }
    rows.push(("SHA-256".to_string(), text(&detail, "contentSha256")));
    rows.push(("Size".to_string(), format!("{} bytes", number(&detail, "bytes"))));
    if s(&detail, "decidedBy").is_some() {
        rows.push((
            "Decided".to_string(),
            format!("{}, {}", text(&detail, "decidedBy"), text(&detail, "decidedAt")),
        ));
        if let Some(reason) = s(&detail, "decisionReason") {
            rows.push(("Decision reason".to_string(), safe_line(reason)));
        }
    }
    if s(&detail, "librarySkillId").is_some() {
        rows.push((
            "Library".to_string(),
            format!(
                "{}, version {}",
                text(&detail, "librarySkillId"),
                number(&detail, "libraryVersion")
            ),
        ));
    }
    if s(&detail, "withdrawnBy").is_some() {
        rows.push((
            "Withdrawn".to_string(),
            format!("{}, {}", text(&detail, "withdrawnBy"), text(&detail, "withdrawnAt")),
        ));
        if let Some(reason) = s(&detail, "withdrawnReason") {
            rows.push(("Withdrawal reason".to_string(), safe_line(reason)));
        }
    }
    cmd.lines(pairs(style, &rows));
    cmd.out("");

    let content = s(&detail, "content").unwrap_or("");
    cmd.out(style.bold("Changes since the last approved version"));
    match detail.get("previous").filter(|previous| !previous.is_null()) {
        None => {
            cmd.out(style.dim(
                "None to show. There is no earlier approved version of this skill, or you do not load its library.",
            ))
        }
        Some(previous) => {
            let before = number(previous, "libraryVersion");
            cmd.out(style.dim(&format!(
                "Library version {before} of {}, approved {}.",
                text(previous, "name"),
                text(previous, "decidedAt")
            )));
            let diff = unified_diff(
                s(previous, "content").unwrap_or(""),
                content,
                &format!("library version {before}"),
                &format!("promotion request {}", text(&detail, "id")),
            );
            if diff.lines.is_empty() {
                cmd.out(format!("No changes: the text is the same as library version {before}."));
            } else {
                for (at, line) in diff.lines.iter().enumerate() {
                    // A carriage return before a line end is part of that line end, not a hazard. The marker line
                    // follows a last line that has no line end, so its carriage return, if any, stays a hazard.
                    let ends_line = at >= 2
                        && matches!(line.as_bytes().first(), Some(b' ' | b'+' | b'-'))
                        && diff.lines.get(at + 1).map(String::as_str) != Some("\\ No newline at end of file");
                    let shown = safe_text(if ends_line {
                        line.strip_suffix('\r').unwrap_or(line)
                    } else {
                        line
                    });
                    // The first two lines are the labels; after them, a line is marked by its first character.
                    cmd.out(match line.as_bytes().first() {
                        _ if at < 2 => style.bold(&shown),
                        Some(b'+') => style.green(&shown),
                        Some(b'-') => style.red(&shown),
                        Some(b'@') => style.cyan(&shown),
                        _ => shown,
                    });
                }
                cmd.out(style.dim(&format!("{} added, {} removed.", diff.added, diff.removed)));
                let (was, is) = (line_ends(s(previous, "content").unwrap_or("")), line_ends(content));
                if was != is {
                    cmd.out(style.dim(&format!("Line ends changed from {was} to {is}.")));
                }
                if diff.coarse {
                    cmd.out(style.dim(
                        "Part of the change was too large to line up: those lines show as removed, then as added.",
                    ));
                }
            }
        }
    }
    cmd.out("");
    cmd.out(style.bold(&format!("Text (SKILL.md, {} bytes)", number(&detail, "bytes"))));
    if content.is_empty() {
        cmd.out("  (empty)");
        return Ok(0);
    }
    let ends = line_ends(content);
    if ends != "LF" {
        cmd.out(style.dim(&format!(
            "Line ends: {ends}. A carriage return that ends a line is not shown."
        )));
    }
    let lines: Vec<&str> = content.strip_suffix('\n').unwrap_or(content).split('\n').collect();
    for (at, line) in lines.iter().enumerate() {
        // Every line has a line end, except the last one of a text that does not end with a newline.
        let line = if at + 1 < lines.len() || content.ends_with('\n') {
            line.strip_suffix('\r').unwrap_or(line)
        } else {
            line
        };
        cmd.out(if line.is_empty() {
            "  |".to_string()
        } else {
            format!("  | {}", safe_text(line))
        });
    }
    Ok(0)
}

// ---------------------------------------------------------------- approve, refuse, withdraw

/// `nexus skills approve <id> [--reason TEXT]` and `nexus skills refuse <id> --reason TEXT`.
pub fn decide(args: &[String], cmd: &Cmd, refuse: bool) -> Result<i32> {
    let usage_line = if refuse {
        "nexus skills refuse <id> --reason TEXT"
    } else {
        "nexus skills approve <id> [--reason TEXT]"
    };
    let id = id_arg(args, usage_line, FIND_REQUEST)?;
    let reason = text_flag(cmd, "reason")?;
    if refuse && reason.is_none() {
        return usage(
            "A refusal needs a reason, so the author knows what to change.",
            Some(usage_line),
        );
    }
    let mut body = json!({ "decision": if refuse { "refused" } else { "approved" } });
    if let Some(reason) = &reason {
        body["reason"] = json!(reason);
    }
    let promotion = cmd
        .core
        .post(&format!("/v1/skill-promotions/{}/decision", enc(id)), body)
        .map_err(refused)?;
    if cmd.json {
        return Ok(print_json(cmd, &promotion));
    }
    let slugs = team_slugs(cmd, std::slice::from_ref(&promotion));
    let name = text(&promotion, "name");
    let version = number(&promotion, "sourceVersion");
    let mut rows = vec![("Request".to_string(), text(&promotion, "id"))];
    if refuse {
        cmd.out(format!("{} Refused {name}, version {version}.", cmd.style.ok));
        rows.push(("Reason".to_string(), safe_line(reason.as_deref().unwrap_or(""))));
    } else {
        cmd.out(format!("{} Approved {name}, version {version}.", cmd.style.ok));
        rows.push((
            "Library".to_string(),
            format!(
                "version {} of {name}, for {}",
                number(&promotion, "libraryVersion"),
                place(&promotion, &slugs, true)
            ),
        ));
        if let Some(reason) = &reason {
            rows.push(("Reason".to_string(), safe_line(reason)));
        }
    }
    cmd.lines(pairs(cmd.style, &rows));
    Ok(0)
}

/// `nexus skills withdraw <id> [--reason TEXT] [--all-versions]`.
pub fn withdraw(args: &[String], cmd: &Cmd) -> Result<i32> {
    let id = id_arg(
        args,
        "nexus skills withdraw <id> [--reason TEXT] [--all-versions]",
        FIND_REQUEST,
    )?;
    let reason = text_flag(cmd, "reason")?;
    let all = cmd.has("all-versions");
    let mut body = json!({});
    if let Some(reason) = &reason {
        body["reason"] = json!(reason);
    }
    if all {
        body["allVersions"] = json!(true);
    }
    let withdrawn = cmd
        .core
        .post(&format!("/v1/skill-promotions/{}/withdraw", enc(id)), body)
        .map_err(refused)?;
    if cmd.json {
        return Ok(print_json(cmd, &withdrawn));
    }
    let name = text(&withdrawn, "name");
    let ids: Vec<String> = a(&withdrawn, "withdrawnIds")
        .iter()
        .filter_map(Value::as_str)
        .map(safe_text)
        .collect();
    let approved = withdrawn.get("libraryVersion").and_then(Value::as_i64);
    let headline = match (ids.len(), approved) {
        (count, _) if count > 1 => format!("Withdrew {count} versions of {name}."),
        (_, Some(version)) => format!("Withdrew {name}, library version {version}."),
        _ => format!(
            "Withdrew the request to promote {name}, version {}.",
            number(&withdrawn, "sourceVersion")
        ),
    };
    cmd.out(format!("{} {headline}", cmd.style.ok));
    let mut rows = vec![(
        if ids.len() > 1 { "Requests" } else { "Request" }.to_string(),
        if ids.is_empty() {
            text(&withdrawn, "id")
        } else {
            ids.join(", ")
        },
    )];
    if let Some(reason) = &reason {
        rows.push(("Reason".to_string(), safe_line(reason)));
    }
    if approved.is_some() {
        rows.push((
            "Effect".to_string(),
            if all {
                "New sessions and chat turns stop loading it. A session that already loaded it keeps the text."
            } else {
                "New sessions and chat turns load the highest approved version that is left, if any. Add --all-versions to stop the whole skill. A session that already loaded it keeps the text."
            }
            .to_string(),
        ));
    }
    cmd.lines(pairs(cmd.style, &rows));
    Ok(0)
}

/// The error a team that is not in the list is: the one Gateway gives for a team it does not know.
pub(crate) fn unknown_team(slug: &str) -> Error {
    refused(Error::Core(Box::new(crate::error::CoreError {
        status: 404,
        message: format!("no team {slug} in this organization; see `nexus team list`"),
        code: Some("unknown_team".into()),
        interval: None,
        details: None,
    })))
}
