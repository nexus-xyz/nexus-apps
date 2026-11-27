//! The coding agents' own conversations on this machine: what Claude Code keeps under `~/.claude/projects` (or
//! `$CLAUDE_CONFIG_DIR/projects`), one JSONL transcript per conversation, and what Codex keeps under
//! `~/.codex/sessions/YYYY/MM/DD` (or `$CODEX_HOME/sessions`), one rollout per conversation, whether they ran through
//! `nexus claude` / `nexus codex` or on their own. Nexus for desktop lists them, newest first, with the command that
//! resumes each in its folder.
//!
//! Read only, and only what a list needs: the title, the folder, the git branch, the model, when it started and last
//! changed, and Claude Code's pull request link. Transcripts grow large (hundreds of MB in all), so only the newest
//! files are opened, and only their first and last [`WINDOW`] bytes. Nothing here leaves the machine.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use serde_json::Value;

use crate::agent_entries::Agent;

/// How much of a transcript's start and end is read.
pub const WINDOW: u64 = 256 * 1024;
/// The longest title shown.
const TITLE_CHARS: usize = 100;

/// One conversation on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSession {
    /// `claude` or `codex`.
    pub agent: String,
    /// The agent's own conversation id: what `--resume` / `resume` takes.
    pub id: String,
    /// Its title: the one the person gave it, else the agent's summary, else the first prompt (one line, shortened).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The folder it ran in, and its last part.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Where Claude Code ran it (`cli`, `claude-desktop`, `sdk-ts`), or what started Codex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// When the transcript last changed (ISO 8601).
    pub updated_at: String,
    /// Claude Code's link to the pull request it opened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// The shell command that resumes it through Nexus, in its folder.
    pub resume: String,
}

/// An id an agent wrote, safe to put in a command: 8 to 64 letters, digits or dashes.
pub fn valid_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// `text` as one single-quoted shell word.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The command that resumes a conversation through Nexus (`nexus claude --resume <id>`, `nexus codex resume <id>`),
/// from its folder when it has one; `None` for an id that is not safe.
pub fn resume_command(agent: Agent, id: &str, cwd: Option<&str>) -> Option<String> {
    if !valid_id(id) {
        return None;
    }
    let resume = format!(
        "nexus {} {}",
        agent.command(),
        agent.entry().session.resume.replace("{id}", id)
    );
    Some(match cwd.filter(|cwd| !cwd.is_empty()) {
        Some(cwd) => format!("cd {} && {resume}", shell_quote(cwd)),
        None => resume,
    })
}

/// The agent's configuration folder: its `profile.var` when set, else its `profile.dir` under the home folder.
fn profile(agent: Agent, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    nexus_local_agents::profile_dir(agent, env)
}

fn claude_projects(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    Some(profile(Agent::Claude, env)?.join("projects"))
}

fn codex_sessions(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    Some(profile(Agent::Codex, env)?.join("sessions"))
}

/// The `.jsonl` files right under `dir`, with when each last changed.
fn transcripts_in(dir: &Path) -> Vec<(SystemTime, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|path| Some((std::fs::metadata(&path).ok()?.modified().ok()?, path)))
        .collect()
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// The JSON lines in the first and last [`WINDOW`] bytes of `path` (the whole file when it is small); a line cut by a
/// window's edge is skipped.
fn head_and_tail(path: &Path) -> (Vec<Value>, Vec<Value>) {
    let Ok(mut file) = File::open(path) else {
        return (vec![], vec![]);
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let mut head = Vec::new();
    let _ = (&mut file).take(WINDOW).read_to_end(&mut head);
    let head_lines = parse_lines(&head, false, len > WINDOW);
    if len <= WINDOW {
        return (head_lines, vec![]);
    }
    let mut tail = Vec::new();
    if file
        .seek(SeekFrom::Start(len.saturating_sub(WINDOW).max(WINDOW)))
        .is_ok()
    {
        let _ = file.read_to_end(&mut tail);
    }
    (head_lines, parse_lines(&tail, true, false))
}

fn parse_lines(bytes: &[u8], skip_first: bool, skip_last: bool) -> Vec<Value> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<&str> = text.split('\n').collect();
    if skip_first && !lines.is_empty() {
        lines.remove(0);
    }
    if skip_last {
        lines.pop();
    }
    lines
        .into_iter()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect()
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(String::from)
}

/// A message's text: a string, or its `text` blocks joined.
fn message_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let joined: Vec<&str> = blocks
                .iter()
                .filter(|block| matches!(block.get("type").and_then(Value::as_str), Some("text" | "input_text")))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect();
            (!joined.is_empty()).then(|| joined.join(" "))
        }
        _ => None,
    }
}

/// A prompt a person typed, not one the agent added (`<command-name>`, `<environment_context>`, AGENTS.md).
fn typed_prompt(text: &str) -> bool {
    let text = text.trim_start();
    !text.is_empty() && !text.starts_with('<') && !text.starts_with("# AGENTS.md") && !text.starts_with("Caveat:")
}

/// One line, at most [`TITLE_CHARS`] characters.
fn one_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= TITLE_CHARS {
        return line;
    }
    let cut: String = line.chars().take(TITLE_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// A time as Gateway writes them: ISO 8601 UTC with milliseconds (`2026-10-19T10:00:01.000Z`).
pub(crate) fn iso(time: SystemTime) -> String {
    let millis = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    let (days, ms) = (millis.div_euclid(86_400_000), millis.rem_euclid(86_400_000));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

fn last_part(cwd: &str) -> Option<String> {
    cwd.split(['/', '\\']).rfind(|part| !part.is_empty()).map(String::from)
}

/// A Claude Code transcript, from its first and last lines.
fn claude_session(path: &Path, modified: SystemTime) -> Option<LocalSession> {
    let id = path.file_stem()?.to_str()?.to_string();
    if !valid_id(&id) {
        return None;
    }
    let (head, tail) = head_and_tail(path);
    let all = || head.iter().chain(tail.iter());
    let kind = |line: &Value| line.get("type").and_then(Value::as_str).map(String::from);
    let first_prompt = head
        .iter()
        .filter(|line| kind(line).as_deref() == Some("user") && line.get("isMeta") != Some(&Value::Bool(true)))
        .filter_map(|line| message_text(line.get("message").and_then(|message| message.get("content"))))
        .find(|text| typed_prompt(text));
    let title = all()
        .filter(|line| kind(line).as_deref() == Some("custom-title"))
        .filter_map(|line| text(line.get("customTitle")))
        .next_back()
        .or_else(|| {
            all()
                .filter(|line| kind(line).as_deref() == Some("summary"))
                .filter_map(|line| text(line.get("summary")))
                .next_back()
        })
        .or(first_prompt)
        .or_else(|| all().filter_map(|line| text(line.get("lastPrompt"))).next_back());
    let cwd = all().filter_map(|line| text(line.get("cwd"))).next_back();
    if title.is_none() && cwd.is_none() {
        return None;
    }
    let model = all()
        .filter(|line| kind(line).as_deref() == Some("assistant"))
        .filter_map(|line| text(line.get("message").and_then(|message| message.get("model"))))
        .rfind(|model| !model.starts_with('<'));
    Some(LocalSession {
        agent: Agent::Claude.as_str().into(),
        resume: resume_command(Agent::Claude, &id, cwd.as_deref())?,
        project: cwd.as_deref().and_then(last_part),
        git_branch: all().filter_map(|line| text(line.get("gitBranch"))).next_back(),
        entrypoint: all().filter_map(|line| text(line.get("entrypoint"))).next_back(),
        started_at: head.iter().find_map(|line| text(line.get("timestamp"))),
        pr_url: all().filter_map(|line| text(line.get("prUrl"))).next_back(),
        title: title.map(|title| one_line(&title)),
        updated_at: iso(modified),
        cwd,
        model,
        id,
    })
}

/// A Codex rollout, from its first lines: `session_meta`, the turn's model, the first prompt.
fn codex_session(path: &Path, modified: SystemTime) -> Option<LocalSession> {
    let (head, _) = head_and_tail(path);
    let payload_of = |kind: &str| {
        head.iter()
            .filter(|line| line.get("type").and_then(Value::as_str) == Some(kind))
            .filter_map(|line| line.get("payload"))
            .collect::<Vec<_>>()
    };
    let meta = *payload_of("session_meta").first()?;
    let id = text(meta.get("id")).or_else(|| text(meta.get("session_id")))?;
    let cwd = text(meta.get("cwd"));
    let first_prompt = payload_of("response_item")
        .into_iter()
        .filter(|item| {
            item.get("type").and_then(Value::as_str) == Some("message")
                && item.get("role").and_then(Value::as_str) == Some("user")
        })
        .filter_map(|item| message_text(item.get("content")))
        .find(|text| typed_prompt(text));
    Some(LocalSession {
        agent: Agent::Codex.as_str().into(),
        resume: resume_command(Agent::Codex, &id, cwd.as_deref())?,
        project: cwd.as_deref().and_then(last_part),
        git_branch: text(meta.get("git").and_then(|git| git.get("branch"))),
        model: payload_of("turn_context")
            .into_iter()
            .find_map(|turn| text(turn.get("model"))),
        entrypoint: text(meta.get("originator")),
        started_at: text(meta.get("timestamp")),
        pr_url: None,
        title: first_prompt.map(|prompt| one_line(&prompt)),
        updated_at: iso(modified),
        cwd,
        id,
    })
}

/// The newest `limit` conversations of every agent, newest first.
pub fn list(env: &dyn Fn(&str) -> Option<String>, limit: usize) -> Vec<LocalSession> {
    let mut files: Vec<(SystemTime, PathBuf, Agent)> = Vec::new();
    for agent in Agent::ALL {
        let found: Vec<(SystemTime, PathBuf)> = match agent {
            Agent::Claude => claude_projects(env)
                .map(|projects| {
                    subdirs(&projects)
                        .iter()
                        .flat_map(|project| transcripts_in(project))
                        .collect()
                })
                .unwrap_or_default(),
            Agent::Codex => codex_sessions(env)
                .map(|sessions| {
                    subdirs(&sessions)
                        .iter()
                        .flat_map(|year| subdirs(year))
                        .flat_map(|month| subdirs(&month))
                        .flat_map(|day| transcripts_in(&day))
                        .collect()
                })
                .unwrap_or_default(),
            // Antigravity keeps each conversation in a database this module does not read yet: none are listed.
            Agent::Antigravity | Agent::Cursor | Agent::Opencode | Agent::Grok => Vec::new(),
        };
        files.extend(found.into_iter().map(|(at, path)| (at, path, agent)));
    }
    files.sort_by_key(|file| std::cmp::Reverse(file.0));
    files
        .into_iter()
        .filter_map(|(modified, path, agent)| match agent {
            Agent::Claude => claude_session(&path, modified),
            Agent::Codex => codex_session(&path, modified),
            Agent::Antigravity | Agent::Cursor | Agent::Opencode | Agent::Grok => None,
        })
        .take(limit)
        .collect()
}

/// One conversation by agent and id, read again from disk (what a resume runs, never what a page sent).
pub fn find(env: &dyn Fn(&str) -> Option<String>, agent: Agent, id: &str) -> Option<LocalSession> {
    if !valid_id(id) {
        return None;
    }
    match agent {
        Agent::Claude => subdirs(&claude_projects(env)?).into_iter().find_map(|project| {
            let path = project.join(format!("{id}.jsonl"));
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            claude_session(&path, modified)
        }),
        Agent::Codex => {
            let sessions = codex_sessions(env)?;
            subdirs(&sessions)
                .iter()
                .flat_map(|year| subdirs(year))
                .flat_map(|month| subdirs(&month))
                .flat_map(|day| transcripts_in(&day))
                .filter(|(_, path)| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.contains(id))
                })
                .find_map(|(modified, path)| codex_session(&path, modified).filter(|session| session.id == id))
        }
        Agent::Antigravity | Agent::Cursor | Agent::Opencode | Agent::Grok => None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn home() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-local-sessions-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn write(path: &Path, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        let mut file = File::create(path).expect("file");
        for line in lines {
            writeln!(file, "{line}").expect("write");
        }
    }

    const CLAUDE_ID: &str = "0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0";
    const CODEX_ID: &str = "01a156ee-7443-7d32-8795-bf2a938d5eda";

    fn seed(home: &Path) {
        let claude = home
            .join(".claude/projects/-Users-ada-web")
            .join(format!("{CLAUDE_ID}.jsonl"));
        write(
            &claude,
            &[
                serde_json::json!({ "type": "user", "isMeta": true, "cwd": "/Users/ada/web", "message": { "role": "user", "content": "<command-name>/init</command-name>" }, "timestamp": "2026-10-19T10:00:00.000Z", "entrypoint": "cli", "gitBranch": "main" }),
                serde_json::json!({ "type": "user", "cwd": "/Users/ada/web", "message": { "role": "user", "content": [{ "type": "text", "text": "Fix the  login\nbug in the header" }] }, "timestamp": "2026-10-19T10:00:01.000Z", "gitBranch": "fix/login" }),
                serde_json::json!({ "type": "assistant", "cwd": "/Users/ada/web", "message": { "role": "assistant", "model": "claude-opus-5-5", "content": [] } }),
                serde_json::json!({ "type": "pr-link", "prUrl": "https://github.com/acme/web/pull/7" }),
            ],
        );
        let codex = home
            .join(".codex/sessions/2026/10/19")
            .join(format!("rollout-2026-10-19T19-30-30-{CODEX_ID}.jsonl"));
        write(
            &codex,
            &[
                serde_json::json!({ "type": "session_meta", "payload": { "id": CODEX_ID, "cwd": "/Users/ada/api", "git": { "branch": "main" }, "timestamp": "2026-10-19T19:30:30.000Z", "originator": "codex_cli_rs" } }),
                serde_json::json!({ "type": "turn_context", "payload": { "model": "gpt-5.3-codex" } }),
                serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "<environment_context>…</environment_context>" }] } }),
                serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Add pagination to /orders" }] } }),
            ],
        );
        // Not a conversation: no id it could resume.
        write(
            &home.join(".claude/projects/-Users-ada-web/notes.jsonl"),
            &[serde_json::json!({ "type": "user" })],
        );
    }

    #[test]
    fn lists_both_agents_conversations_with_what_resumes_them() {
        let home = home();
        seed(&home);
        let dir = home.to_string_lossy().into_owned();
        let env = |name: &str| (name == "HOME").then(|| dir.clone());
        let sessions = list(&env, 10);
        assert_eq!(sessions.len(), 2, "{sessions:?}");
        let claude = sessions
            .iter()
            .find(|session| session.agent == "claude")
            .expect("claude");
        assert_eq!(claude.id, CLAUDE_ID);
        // The first prompt a person typed, on one line; not the slash command Claude Code recorded first.
        assert_eq!(claude.title.as_deref(), Some("Fix the login bug in the header"));
        assert_eq!(claude.project.as_deref(), Some("web"));
        assert_eq!(claude.git_branch.as_deref(), Some("fix/login"));
        assert_eq!(claude.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(claude.entrypoint.as_deref(), Some("cli"));
        assert_eq!(claude.pr_url.as_deref(), Some("https://github.com/acme/web/pull/7"));
        assert_eq!(
            claude.resume,
            format!("cd '/Users/ada/web' && nexus claude --resume {CLAUDE_ID}")
        );
        let codex = sessions.iter().find(|session| session.agent == "codex").expect("codex");
        assert_eq!(codex.title.as_deref(), Some("Add pagination to /orders"));
        assert_eq!(codex.model.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(
            codex.resume,
            format!("cd '/Users/ada/api' && nexus codex resume {CODEX_ID}")
        );
        assert_eq!(list(&env, 1).len(), 1);
        assert_eq!(
            find(&env, Agent::Codex, CODEX_ID).map(|session| session.cwd),
            Some(Some("/Users/ada/api".into()))
        );
        assert_eq!(find(&env, Agent::Claude, "../../etc/passwd"), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_large_transcript_is_read_at_its_ends_only() {
        let home = home();
        let path = home.join(".claude/projects/-big").join(format!("{CLAUDE_ID}.jsonl"));
        let filler = "x".repeat(1024);
        let mut lines =
            vec![serde_json::json!({ "type": "user", "cwd": "/big", "message": { "content": "first prompt" } })];
        lines.extend((0..600).map(|_| serde_json::json!({ "type": "attachment", "rendered": filler })));
        lines.push(serde_json::json!({ "type": "custom-title", "customTitle": "Named at the end" }));
        write(&path, &lines);
        assert!(std::fs::metadata(&path).expect("meta").len() > 2 * WINDOW);
        let dir = home.to_string_lossy().into_owned();
        let env = |name: &str| (name == "HOME").then(|| dir.clone());
        let session = find(&env, Agent::Claude, CLAUDE_ID).expect("session");
        assert_eq!(session.title.as_deref(), Some("Named at the end"));
        assert_eq!(session.cwd.as_deref(), Some("/big"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn commands_quote_the_folder_and_refuse_unsafe_ids() {
        assert_eq!(shell_quote("/a b/it's"), r"'/a b/it'\''s'");
        assert_eq!(
            resume_command(Agent::Claude, "abc12345", Some("/x y")).as_deref(),
            Some("cd '/x y' && nexus claude --resume abc12345")
        );
        assert_eq!(
            resume_command(Agent::Codex, "abc12345", None).as_deref(),
            Some("nexus codex resume abc12345")
        );
        assert_eq!(resume_command(Agent::Claude, "abc; rm -rf /", None), None);
        assert_eq!(one_line(&"word ".repeat(40)).chars().count(), TITLE_CHARS);
    }

    #[test]
    fn times_read_as_gateway_writes_them() {
        let at = |millis: u64| iso(SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(millis));
        assert_eq!(at(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(at(1_792_229_400_123), "2026-10-17T09:30:00.123Z");
        assert_eq!(at(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }
}
