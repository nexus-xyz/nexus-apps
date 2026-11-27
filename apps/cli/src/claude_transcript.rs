//! A Claude Code session's transcript, read for `nexus agents share` (`claude-transcript.ts`): where it
//! is, and the snapshot it makes for `POST /v1/artifacts/sessions`. Both CLIs read `fixtures/claude-code/` in their
//! tests and must make the same snapshot.
//!
//! Claude Code writes one JSON record per line to `<CLAUDE_CONFIG_DIR or ~/.claude>/projects/<project>/<id>.jsonl`,
//! where `<project>` is the folder it ran in with every character but letters and digits made `-`. Records are read
//! in the order written:
//!
//! - `user`: a prompt (its text and images), or tool results. A result the person was asked about gets an approval
//!   and its decision before it (`permissionDecision` from the person, or Claude Code's "doesn't want to proceed").
//!   "[Request interrupted by user]" becomes a notice. Claude Code's own text (`isMeta`, the compact summary) is left
//!   out.
//! - `assistant`: text, tool calls and (with `include_thinking`) thinking. An API error becomes an error event.
//! - `system`: a compaction becomes a notice and an API error an error event.
//! - `ai-title`, `custom-title` and `summary` give the title (custom first); `cost-state` the cost and start.
//!
//! A sub-agent's records (`isSidechain`) are left out, as decided for v1. A record written twice is read once
//! (`uuid`). Every branch of a rewound session stays in, in the order written. The record types in [`UNSHOWN`] hold
//! nothing a reader sees. A record of any other type, or a line that is not JSON, is skipped and counted.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::Context;
use crate::error::{CliError, Result};
pub use nexus_client::local_agents::agents::{claude_project_name, is_session_id};

/// Record types Claude Code writes that hold nothing a reader of the session sees: read, not counted as skipped.
pub const UNSHOWN: [&str; 12] = [
    "attachment",
    "api-request",
    "api-request-blob",
    "api-request-shape",
    "atis-latch",
    "file-history-snapshot",
    "last-prompt",
    "mode",
    "pr-link",
    "progress",
    "queue-operation",
    "tools-used",
];

/// What Claude Code writes as a tool result when the person said no.
const REJECTED: &str = "The user doesn't want to proceed with this tool use";
const INTERRUPTED: &str = "[Request interrupted by user";
const COMPACTED: &str = "Earlier messages were summarized to free up context.";

/// The snapshot of one transcript, and what it skipped.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSnapshot {
    /// `SessionSnapshotInput`: source, id, agent, title, model, cost, start and events.
    pub session: Value,
    /// `SessionShareImage`s: `{ id, data }`, base64.
    pub images: Vec<Value>,
    /// Records of a type this CLI does not read, and lines that are not JSON.
    pub skipped: usize,
}

fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn is_true(value: &Value, key: &str) -> bool {
    value.get(key) == Some(&Value::Bool(true))
}

/// An event with `at` first when the record has a time, as the TypeScript writes it.
fn event(at: Option<i64>, fields: Value) -> Value {
    let mut map = Map::new();
    if let Some(at) = at {
        map.insert("at".into(), json!(at));
    }
    if let Value::Object(fields) = fields {
        map.extend(fields);
    }
    Value::Object(map)
}

/// A record's time in milliseconds (`Date.parse`).
fn millis(timestamp: Option<&str>) -> Option<i64> {
    let parsed = time::OffsetDateTime::parse(timestamp?, &time::format_description::well_known::Rfc3339).ok()?;
    i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).ok()
}

/// A message's content as blocks: a string is one text block.
fn blocks(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![json!({ "type": "text", "text": text })],
        Some(Value::Array(items)) => items.iter().filter(|item| item.is_object()).cloned().collect(),
        _ => Vec::new(),
    }
}

/// An image block's base64 added to `images` under the next id (`img_1`, `img_2`…); `None` when it holds none.
fn image(block: &Value, images: &mut Vec<Value>) -> Option<String> {
    let source = block.get("source")?;
    if str_of(source, "type") != Some("base64") {
        return None;
    }
    let data = str_of(source, "data")?;
    let image_id = format!("img_{}", images.len() + 1);
    images.push(json!({ "id": image_id, "data": data }));
    Some(image_id)
}

/// The snapshot `nexus agents share` sends for the transcript `jsonl` of session `id`.
pub fn read_claude_transcript(id: &str, jsonl: &str, include_thinking: bool) -> TranscriptSnapshot {
    let mut events: Vec<Value> = Vec::new();
    let mut images: Vec<Value> = Vec::new();
    let mut tools: HashMap<String, String> = HashMap::new();
    // Each model's count, and the order it was first seen in (a tie goes to the first, as the TypeScript's sort).
    let mut models: Vec<(String, usize)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let (mut custom, mut ai, mut summary): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    let mut skipped = 0;
    let mut cost: Option<f64> = None;
    let mut recorded_cost: Option<f64> = None;
    let mut started_at: Option<Value> = None;
    let mut first_at: Option<i64> = None;

    for line in jsonl.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            skipped += 1;
            continue;
        };
        if !record.is_object() {
            skipped += 1;
            continue;
        }
        let kind = str_of(&record, "type").unwrap_or("");
        if let Some(uuid) = str_of(&record, "uuid")
            && !seen.insert(uuid.to_string())
        {
            continue;
        }
        let at = millis(str_of(&record, "timestamp"));
        if at.is_some() && (kind == "user" || kind == "assistant") {
            first_at = first_at.or(at);
        }

        if UNSHOWN.contains(&kind) {
            continue;
        }
        match kind {
            "ai-title" => {
                ai = str_of(&record, "aiTitle").map(String::from).or(ai);
                continue;
            }
            "custom-title" => {
                custom = str_of(&record, "customTitle").map(String::from).or(custom);
                continue;
            }
            "summary" => {
                summary = str_of(&record, "summary").map(String::from).or(summary);
                continue;
            }
            "cost-state" => {
                if let Some(total) = record.get("totalCostUSD").and_then(Value::as_f64) {
                    cost = Some(total);
                }
                if let Some(start) = record.get("startTime").filter(|start| start.is_number()) {
                    started_at = Some(start.clone());
                }
                continue;
            }
            "system" => {
                if is_true(&record, "isSidechain") {
                    continue;
                }
                let subtype = str_of(&record, "subtype");
                if subtype == Some("compact_boundary") {
                    events.push(event(at, json!({ "kind": "notice", "text": COMPACTED })));
                } else if subtype == Some("api_error") || str_of(&record, "level") == Some("error") {
                    let text = str_of(&record, "content").unwrap_or("Claude Code reported an error.");
                    events.push(event(at, json!({ "kind": "error", "text": text })));
                } else {
                    skipped += 1;
                }
                continue;
            }
            "user" | "assistant" => {}
            _ => {
                skipped += 1;
                continue;
            }
        }
        // A sub-agent's records, and Claude Code's own text, are not the session's.
        if is_true(&record, "isSidechain") || is_true(&record, "isMeta") || is_true(&record, "isCompactSummary") {
            continue;
        }
        let message = record.get("message").filter(|message| message.is_object());
        let content = blocks(message.and_then(|message| message.get("content")));

        if kind == "assistant" {
            if is_true(&record, "isApiErrorMessage") {
                let said = content
                    .iter()
                    .map(|block| str_of(block, "text").unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join("\n");
                let said = said.trim();
                let text = if said.is_empty() {
                    "The API answered with an error."
                } else {
                    said
                };
                events.push(event(at, json!({ "kind": "error", "text": text })));
                continue;
            }
            if let Some(model) = message.and_then(|message| str_of(message, "model"))
                && !model.is_empty()
                && model != "<synthetic>"
            {
                match models.iter_mut().find(|(name, _)| name == model) {
                    Some((_, count)) => *count += 1,
                    None => models.push((model.to_string(), 1)),
                }
            }
            if let Some(spent) = record.get("costUSD").and_then(Value::as_f64) {
                recorded_cost = Some(recorded_cost.unwrap_or(0.0) + spent);
            }
            for block in &content {
                match str_of(block, "type") {
                    Some("text") => {
                        if let Some(text) = str_of(block, "text").filter(|text| !text.trim().is_empty()) {
                            events.push(event(at, json!({ "kind": "text", "text": text })));
                        }
                    }
                    Some("thinking") if include_thinking => {
                        if let Some(text) = str_of(block, "thinking").filter(|text| !text.trim().is_empty()) {
                            events.push(event(at, json!({ "kind": "thinking", "text": text })));
                        }
                    }
                    Some("tool_use") => {
                        let Some(tool_id) = str_of(block, "id") else { continue };
                        let name = str_of(block, "name").unwrap_or("tool");
                        tools.insert(tool_id.to_string(), name.to_string());
                        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                        events.push(event(
                            at,
                            json!({ "kind": "tool", "id": tool_id, "name": name, "input": input }),
                        ));
                    }
                    _ => {}
                }
            }
            continue;
        }

        let mut prompt: Vec<&str> = Vec::new();
        let mut attached: Vec<String> = Vec::new();
        for block in &content {
            match str_of(block, "type") {
                Some("text") => {
                    let Some(text) = str_of(block, "text") else { continue };
                    if text.starts_with(INTERRUPTED) {
                        events.push(event(
                            at,
                            json!({ "kind": "notice", "text": "Interrupted by the user." }),
                        ));
                    } else {
                        prompt.push(text);
                    }
                }
                Some("image") => {
                    if let Some(image_id) = image(block, &mut images) {
                        attached.push(image_id);
                    }
                }
                Some("tool_result") => {
                    let Some(tool_id) = str_of(block, "tool_use_id") else {
                        continue;
                    };
                    let parts = blocks(block.get("content"));
                    let output = parts
                        .iter()
                        .filter(|part| str_of(part, "type") == Some("text"))
                        .filter_map(|part| str_of(part, "text"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let shots: Vec<String> = parts
                        .iter()
                        .filter(|part| str_of(part, "type") == Some("image"))
                        .filter_map(|part| image(part, &mut images))
                        .collect();
                    let decision = record.get("permissionDecision").filter(|decision| decision.is_object());
                    let asked = decision
                        .and_then(|decision| str_of(decision, "source"))
                        .is_some_and(|source| source.starts_with("user"));
                    let rejected = output.starts_with(REJECTED);
                    if asked || rejected {
                        let tool = tools.get(tool_id).map_or("tool", String::as_str);
                        events.push(event(
                            at,
                            json!({ "kind": "approval", "requestId": tool_id, "toolUseId": tool_id, "tool": tool }),
                        ));
                        let accepted = decision.and_then(|decision| str_of(decision, "decision")) == Some("accept");
                        let scope = decision.and_then(|decision| decision.get("grantScope"));
                        let verdict = if rejected || !accepted {
                            "deny"
                        } else if scope.is_none() || scope == Some(&json!("once")) {
                            "allow"
                        } else {
                            "allowSession"
                        };
                        events.push(event(
                            at,
                            json!({ "kind": "approvalResolved", "requestId": tool_id, "decision": verdict }),
                        ));
                    }
                    let mut result = json!({ "kind": "toolResult", "id": tool_id, "output": output });
                    if is_true(block, "is_error") {
                        result["isError"] = json!(true);
                    }
                    if !shots.is_empty() {
                        result["images"] = json!(shots);
                    }
                    events.push(event(at, result));
                }
                _ => {}
            }
        }
        let said = prompt.join("\n\n");
        if !said.trim().is_empty() || !attached.is_empty() {
            let mut user = json!({ "kind": "user", "text": said });
            if !attached.is_empty() {
                user["images"] = json!(attached);
            }
            events.push(event(at, user));
        }
    }

    let model = models
        .iter()
        .enumerate()
        .max_by(|(i, (_, a)), (j, (_, b))| a.cmp(b).then(j.cmp(i)))
        .map(|(_, (name, _))| name.clone());
    let title = custom.or(ai).or(summary);
    let mut session = json!({ "source": "claude-code", "id": id, "agent": "claude" });
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        session["title"] = json!(title);
    }
    if let Some(model) = model {
        session["model"] = json!(model);
    }
    if let Some(cost) = cost.or(recorded_cost) {
        session["costUsd"] = json!(cost);
    }
    if let Some(started) = started_at.or_else(|| first_at.map(|at| json!(at))) {
        session["startedAt"] = started;
    }
    session["events"] = Value::Array(events);
    TranscriptSnapshot {
        session,
        images,
        skipped,
    }
}

/// Where Claude Code keeps its transcripts: `<CLAUDE_CONFIG_DIR or ~/.claude>/projects`.
pub fn claude_projects_dir(ctx: &Context) -> Result<PathBuf> {
    let set = |name: &str| ctx.var(name).filter(|value| !value.trim().is_empty());
    if let Some(config) = set("CLAUDE_CONFIG_DIR") {
        return Ok(PathBuf::from(config).join("projects"));
    }
    let Some(home) = set("HOME").or_else(|| set("USERPROFILE")) else {
        return Err(CliError::hint(
            "Nexus cannot find Claude Code's sessions: neither CLAUDE_CONFIG_DIR nor HOME is set.",
            "Set CLAUDE_CONFIG_DIR to the folder Claude Code keeps its settings in.",
        )
        .into());
    };
    Ok(PathBuf::from(home).join(".claude").join("projects"))
}

/// The transcript of session `which`: `latest`, this folder's newest; else the session's id, looked for in this
/// folder's project first, then in every project. Answers the session's id and the file.
pub fn find_claude_transcript(which: &str, ctx: &Context) -> Result<(String, PathBuf)> {
    let root = claude_projects_dir(ctx)?;
    let here = root.join(claude_project_name(&ctx.cwd));
    if which == "latest" {
        let mut newest: Option<(String, PathBuf, std::time::SystemTime)> = None;
        for entry in std::fs::read_dir(&here).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".jsonl").filter(|id| is_session_id(id)) else {
                continue;
            };
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let Ok(at) = meta.modified() else { continue };
            if newest.as_ref().is_none_or(|(_, _, newest)| at > *newest) {
                newest = Some((id.to_string(), entry.path(), at));
            }
        }
        return newest.map(|(id, path, _)| (id, path)).ok_or_else(|| {
            CliError::hint(
                format!("No Claude Code session in {}.", ctx.cwd.display()),
                "Run it in the folder you ran nexus claude in, or give the session's id (claude --resume lists them).",
            )
            .into()
        });
    }
    if !is_session_id(which) {
        let mut error = CliError::hint(
            format!("{which} is not a Claude Code session id."),
            "Give the session's id (claude --resume lists them), or latest.",
        );
        error.exit_code = 2;
        return Err(error.into());
    }
    let file = format!("{which}.jsonl");
    // A plain file: a symbolic link is not followed, so a transcript is always a file Claude Code wrote.
    let is_file = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file());
    if is_file(&here.join(&file)) {
        return Ok((which.to_string(), here.join(file)));
    }
    let mut projects: Vec<PathBuf> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    projects.sort();
    for project in projects {
        let path = project.join(&file);
        if is_file(&path) {
            return Ok((which.to_string(), path));
        }
    }
    Err(CliError::hint(
        format!("No Claude Code session {which} on this machine."),
        format!(
            "Nexus looked in {}. claude --resume lists your sessions.",
            root.display()
        ),
    )
    .into())
}

/// The transcript of session `which`, read and made a snapshot, with the file it came from.
pub fn load_claude_session(
    which: &str,
    ctx: &Context,
    include_thinking: bool,
) -> Result<(TranscriptSnapshot, PathBuf)> {
    let (id, path) = find_claude_transcript(which, ctx)?;
    let jsonl = std::fs::read_to_string(&path)
        .map_err(|error| CliError::new(format!("Cannot read {}: {error}", path.display())))?;
    Ok((read_claude_transcript(&id, &jsonl, include_thinking), path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "5f0c2b1e-0000-4000-8000-000000000001";
    const TRANSCRIPT: &str = include_str!("../tests/fixtures/interfaces/claude-code/session.jsonl");
    const SNAPSHOT: &str = include_str!("../tests/fixtures/interfaces/claude-code/session.snapshot.json");

    #[test]
    fn makes_the_snapshot_both_clis_make() {
        let read = read_claude_transcript(SESSION, TRANSCRIPT, false);
        let expected: Value = serde_json::from_str(SNAPSHOT).expect("snapshot");
        assert_eq!(
            json!({ "session": read.session, "images": read.images, "skipped": read.skipped }),
            expected
        );
    }

    #[test]
    fn keeps_thinking_only_when_asked() {
        let events = read_claude_transcript(SESSION, TRANSCRIPT, true).session["events"].clone();
        assert_eq!(
            events[1],
            json!({ "at": 1_792_490_401_000_i64, "kind": "thinking", "text": "Look at the build log first." })
        );
    }

    #[test]
    fn names_a_custom_title_first_and_sums_recorded_costs() {
        let lines = [
            json!({ "type": "summary", "summary": "Old summary" }),
            json!({ "type": "custom-title", "customTitle": "My title" }),
            json!({ "type": "user", "uuid": "u", "timestamp": "2026-10-20T10:00:00.000Z", "message": { "role": "user", "content": "hi" } }),
            json!({ "type": "assistant", "uuid": "a", "costUSD": 0.25, "message": { "model": "claude-haiku-5-5", "content": [{ "type": "text", "text": "hello" }] } }),
            json!({ "type": "assistant", "uuid": "b", "costUSD": 0.5, "message": { "model": "claude-haiku-5-5", "content": [] } }),
        ];
        let jsonl = lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
        let read = read_claude_transcript("s", &jsonl, false);
        assert_eq!(read.session["title"], "My title");
        assert_eq!(read.session["costUsd"], 0.75);
        assert_eq!(read.session["model"], "claude-haiku-5-5");
        assert_eq!(read.session["startedAt"], 1_792_490_400_000_i64);
        assert_eq!(read.skipped, 0);
    }

    #[test]
    fn session_ids_are_never_paths() {
        assert!(is_session_id(SESSION));
        assert!(is_session_id("latest"));
        for bad in ["", "../etc/passwd", "a/b", "-x", ".hidden"] {
            assert!(!is_session_id(bad), "{bad}");
        }
        assert_eq!(claude_project_name(Path::new("/work/hello.app")), "-work-hello-app");
    }
}
